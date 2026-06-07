//! RDP session actor (Stage 4, round 2b-1) — read-only live desktop.
//!
//! Reuses the EXACT connect path validated by `examples/rdp_spike.rs`
//! (blocking IronRDP: connect → CredSSP → ActiveStage). Because that path
//! is blocking, we run it on a dedicated OS thread and bridge to async:
//!   - **out**: the worker emits `RdpSessionEvent`s straight through the
//!     Tokio `UnboundedSender` (its `send` is sync, callable off-runtime);
//!   - **in**: a shared `AtomicBool` shutdown flag the async side flips when
//!     the UI closes the tab; the worker checks it between reads.
//!
//! Round 2b-2 adds input (mouse/keyboard/modifier-sync) — that needs the
//! IronRDP `input` API the spike didn't exercise, so it gets its own pass.
//! For now input commands are accepted and dropped.
//!
//! Frame strategy (MVP): push the whole framebuffer, throttled to ~10 fps.
//! Region-diffing + a faster transport than JSON are the documented
//! follow-up (see `docs/specs/rdp-session.md`, Open-Q #1).

use std::io::Write as _;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument};

use ironrdp::connector::{self, ConnectionResult, Credentials};
use ironrdp::input::{Database, MouseButton as IrMouseButton, MousePosition, Operation, WheelRotations};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageOutput};
use sspi::network_client::reqwest_network_client::ReqwestNetworkClient;
use tokio_rustls::rustls;

use crate::{
    ImageFormat, MouseButton, RdpCloseReason, RdpCommand, RdpError, RdpInputEvent,
    RdpSessionEvent, RdpSpawnParams, RdpState, RevealedRdpCredential,
};

const CMD_CHANNEL_CAP: usize = 256;
/// Per-read socket timeout during the active stage. Small so the worker
/// drains UI input (and notices shutdown) promptly — this is the dominant
/// input-latency knob. read_pdu still returns immediately when data is
/// available; this only bounds the idle wait.
const READ_POLL: Duration = Duration::from_millis(16);
/// Generous timeout covering the (multi-roundtrip) connect handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Frame diff/emit tick (~30 fps target). Encoding runs on a separate thread
/// and frames coalesce, so aiming higher than the old 15 fps is safe: when the
/// encoder can't keep up, frames are simply skipped rather than queued.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// JPEG quality for frame transport (0-100). Desktop content is mostly
/// text/UI; 72 blurred small text noticeably, so we run a bit higher.
/// Region-diff keeps payloads small enough that this is affordable.
const JPEG_QUALITY: u8 = 85;
/// Height of each diff/encode band. The screen is sliced into bands so that
/// two far-apart changes don't merge into one near-fullscreen rectangle
/// (which caused periodic lag spikes). 64px ≈ a toolbar/row height.
const BAND_H: usize = 64;
/// Regions up to this pixel area are sent as PNG (lossless → crisp text);
/// larger regions fall back to JPEG (compact + fast). Kept small on purpose:
/// PNG is lossless but its encode cost grows with area and would block the
/// worker on big regions. Small popups, carets, icons, short text runs go
/// PNG; toolbars / big repaints / wallpaper go JPEG.
const PNG_MAX_AREA: usize = 40_000;

/// Spawn an RDP session actor. Returns the command sender and the task
/// handle (mirrors `rh_ssh::spawn_session`).
pub fn spawn_session(
    params: RdpSpawnParams,
    events: mpsc::UnboundedSender<RdpSessionEvent>,
) -> (mpsc::Sender<RdpCommand>, JoinHandle<()>) {
    let (tx_cmd, rx_cmd) = mpsc::channel::<RdpCommand>(CMD_CHANNEL_CAP);
    let join = tokio::spawn(run(params, rx_cmd, events));
    (tx_cmd, join)
}

#[instrument(level = "debug", skip(params, rx_cmd, events), fields(session_id = %params.id))]
async fn run(
    params: RdpSpawnParams,
    mut rx_cmd: mpsc::Receiver<RdpCommand>,
    events: mpsc::UnboundedSender<RdpSessionEvent>,
) {
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_shutdown = shutdown.clone();
    let worker_events = events.clone();
    // Bridge UI input to the blocking worker: a std channel the worker
    // drains between reads. (Shutdown still travels via the atomic flag.)
    let (input_tx, input_rx) = std::sync::mpsc::channel::<RdpInputEvent>();

    let worker = match std::thread::Builder::new()
        .name("rdp-session".to_owned())
        .spawn(move || blocking_session(params, &worker_shutdown, &worker_events, &input_rx))
    {
        Ok(h) => h,
        Err(e) => {
            let _ = events.send(RdpSessionEvent::Error {
                message: format!("failed to spawn RDP worker: {e}"),
            });
            let _ = events.send(RdpSessionEvent::Closed {
                reason: RdpCloseReason::Error,
            });
            return;
        }
    };

    // Forward commands until the UI closes the tab. Input goes to the
    // worker; Resize is a follow-up (2b-3).
    loop {
        match rx_cmd.recv().await {
            Some(RdpCommand::Shutdown) | None => break,
            Some(RdpCommand::Input(ev)) => {
                let _ = input_tx.send(ev);
            }
            Some(RdpCommand::Resize { .. }) => {}
        }
    }

    shutdown.store(true, Ordering::SeqCst);
    // Join the worker without blocking the runtime.
    let _ = tokio::task::spawn_blocking(move || worker.join()).await;
}

/// The blocking worker: connect, then pump the active stage until the
/// server disconnects or the UI requests shutdown.
fn blocking_session(
    params: RdpSpawnParams,
    shutdown: &AtomicBool,
    events: &mpsc::UnboundedSender<RdpSessionEvent>,
    input_rx: &std::sync::mpsc::Receiver<RdpInputEvent>,
) {
    let emit = |e: RdpSessionEvent| {
        let _ = events.send(e);
    };

    let (username, domain, password) = match params.credential {
        RevealedRdpCredential::Password {
            username,
            domain,
            password,
        } => (
            username,
            domain,
            password.as_str().unwrap_or_default().to_owned(),
        ),
    };

    let config = build_config(username, domain, password, &params.options);

    // connect() emits the real phase transitions (Resolving → Connecting →
    // Authenticating) as it progresses, so the UI reflects what's actually
    // happening instead of claiming "Authenticating" before we've even
    // reached the host.
    let (connection_result, mut framed) =
        match connect(config, params.host.hostname.clone(), params.host.port, events) {
            Ok(v) => v,
            Err(e) => {
                let reason = close_reason_for(&e);
                emit(RdpSessionEvent::Error {
                    message: e.to_string(),
                });
                emit(RdpSessionEvent::Closed { reason });
                return;
            }
        };

    debug!(
        width = connection_result.desktop_size.width,
        height = connection_result.desktop_size.height,
        "rdp connected"
    );

    let mut image = DecodedImage::new(
        ironrdp::graphics::image_processing::PixelFormat::RgbA32,
        connection_result.desktop_size.width,
        connection_result.desktop_size.height,
    );
    let mut active_stage = ActiveStage::new(connection_result);
    let mut input_db = Database::new();

    // Tell the UI the real negotiated size so it sizes its canvas to match
    // (the server may clamp/adjust what we requested). Region coordinates
    // are in this space.
    emit(RdpSessionEvent::Resized {
        width: image.width(),
        height: image.height(),
    });
    emit(RdpSessionEvent::StateChanged {
        state: RdpState::Ready,
    });

    // Off-thread encoding: the worker only diffs + extracts changed-region
    // pixels (cheap), then hands them to this encoder thread, which does the
    // expensive JPEG/PNG + base64 and emits the frame. This keeps the
    // read/input loop off the critical path — input stays responsive (~16ms)
    // regardless of how long a big frame takes to compress. Capacity 1 +
    // try_send means frames coalesce (latest wins) when the encoder can't
    // keep up, instead of building a backlog.
    let (tx_enc, rx_enc) = std::sync::mpsc::sync_channel::<Vec<RegionJob>>(1);
    let enc_events: mpsc::UnboundedSender<RdpSessionEvent> = events.clone();
    let enc_handle = std::thread::spawn(move || encoder_loop(rx_enc, &enc_events));

    let mut last_emit = Instant::now()
        .checked_sub(FRAME_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut last_frame: Vec<u8> = Vec::new();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            emit(RdpSessionEvent::Closed {
                reason: RdpCloseReason::UserRequested,
            });
            return;
        }

        // Drain all pending UI input (non-blocking). Coalesce consecutive
        // mouse-moves — only the latest position matters, so a flood of
        // moves can't make a click (or the read loop) fall behind.
        let mut pending: Vec<RdpInputEvent> = Vec::new();
        while let Ok(ev) = input_rx.try_recv() {
            pending.push(ev);
        }
        for (i, ev) in pending.iter().enumerate() {
            // Skip a move that is immediately superseded by another move.
            if matches!(ev, RdpInputEvent::MouseMove { .. })
                && matches!(pending.get(i + 1), Some(RdpInputEvent::MouseMove { .. }))
            {
                continue;
            }
            if send_input(&mut active_stage, &mut input_db, &mut framed, &mut image, ev.clone())
                .is_err()
            {
                emit(RdpSessionEvent::Closed {
                    reason: RdpCloseReason::ServerDisconnected { code: None },
                });
                return;
            }
        }

        let (action, payload) = match framed.read_pdu() {
            Ok(v) => v,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => {
                emit(RdpSessionEvent::Closed {
                    reason: RdpCloseReason::ServerDisconnected { code: None },
                });
                return;
            }
        };

        let outputs = match active_stage.process(&mut image, action, &payload) {
            Ok(o) => o,
            Err(e) => {
                emit(RdpSessionEvent::Error {
                    message: format!("active stage: {e}"),
                });
                emit(RdpSessionEvent::Closed {
                    reason: RdpCloseReason::Error,
                });
                return;
            }
        };

        for out in outputs {
            match out {
                ActiveStageOutput::ResponseFrame(frame) => {
                    if framed.write_all(&frame).is_err() {
                        emit(RdpSessionEvent::Closed {
                            reason: RdpCloseReason::ServerDisconnected { code: None },
                        });
                        return;
                    }
                }
                ActiveStageOutput::Terminate(_) => {
                    emit(RdpSessionEvent::Closed {
                        reason: RdpCloseReason::ServerDisconnected { code: None },
                    });
                    return;
                }
                _ => {}
            }
        }

        // Diff + extract changed regions (cheap), then hand the raw pixels
        // to the encoder thread. If the encoder is still busy with the
        // previous frame, skip this one (don't advance `last_frame`) so the
        // next diff picks up the accumulated changes — frames coalesce, no
        // backlog, no blocking the read/input loop on compression.
        if last_emit.elapsed() >= FRAME_INTERVAL {
            if let Some(regions) = compute_regions(&image, &last_frame) {
                match tx_enc.try_send(regions) {
                    Ok(()) => {
                        last_frame.clear();
                        last_frame.extend_from_slice(image.data());
                    }
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        // encoder busy — leave last_frame as-is, retry next tick
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
            last_emit = Instant::now();
        }
    }

    // Stop the encoder thread (channel closes when tx_enc drops) and wait so
    // we don't emit a stray frame after Closed.
    drop(tx_enc);
    let _ = enc_handle.join();
}

/// One changed rectangle's pixels, ready to compress on the encoder thread.
struct RegionJob {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    rgb: Vec<u8>,
}

/// Encoder thread: compress each region (PNG for small/text, JPEG for large)
/// and emit it. Runs off the read/input loop so compression never adds input
/// latency. Logs throughput once per second.
fn encoder_loop(
    rx: std::sync::mpsc::Receiver<Vec<RegionJob>>,
    events: &mpsc::UnboundedSender<RdpSessionEvent>,
) {
    let mut stat_window = Instant::now();
    let mut stat_frames: u32 = 0;
    let mut stat_micros: u128 = 0;
    let mut stat_max_micros: u128 = 0;
    let mut stat_bytes: usize = 0;

    while let Ok(regions) = rx.recv() {
        let t = Instant::now();
        // Compress the frame's tiles in parallel across all cores — the main
        // throughput lever. Order within a frame doesn't matter (tiles are
        // non-overlapping rects; the UI draws them all together).
        use rayon::prelude::*;
        let tiles: Vec<crate::FrameTile> = regions
            .into_par_iter()
            .filter_map(|r| {
                let (format, encoded) = if (r.w as usize) * (r.h as usize) <= PNG_MAX_AREA {
                    (ImageFormat::Png, encode_png_base64(&r.rgb, u32::from(r.w), u32::from(r.h)))
                } else {
                    (ImageFormat::Jpeg, encode_jpeg_base64(&r.rgb, r.w, r.h))
                };
                encoded.map(|base64| crate::FrameTile {
                    x: r.x,
                    y: r.y,
                    width: r.w,
                    height: r.h,
                    format,
                    base64,
                })
            })
            .collect();

        let job_bytes: usize = tiles.iter().map(|t| t.base64.len()).sum();
        // Emit the whole frame at once → the UI draws all tiles in one pass,
        // so the frame is presented coherently (no band-by-band tearing).
        if !tiles.is_empty() {
            let _ = events.send(RdpSessionEvent::FrameBatch { tiles });
        }
        let us = t.elapsed().as_micros();
        stat_frames += 1;
        stat_micros += us;
        stat_max_micros = stat_max_micros.max(us);
        stat_bytes += job_bytes;

        if stat_window.elapsed() >= Duration::from_secs(1) {
            let avg = if stat_frames > 0 {
                (stat_micros as f64) / f64::from(stat_frames) / 1000.0
            } else {
                0.0
            };
            info!(
                fps = stat_frames,
                avg_encode_ms = format!("{avg:.1}"),
                max_encode_ms = format!("{:.1}", (stat_max_micros as f64) / 1000.0),
                total_kb = stat_bytes / 1024,
                "rdp frame stats"
            );
            stat_frames = 0;
            stat_micros = 0;
            stat_max_micros = 0;
            stat_bytes = 0;
            stat_window = Instant::now();
        }
    }
}

/// Compute the changed regions since `last_frame` and extract their pixels
/// into owned `RegionJob`s (ready for the encoder thread). Does NOT mutate
/// `last_frame` — the caller advances it only once the regions are handed off
/// (so a skipped frame's changes roll into the next diff).
///
/// The screen is sliced into horizontal bands diffed independently, so two
/// far-apart changes don't merge into one near-fullscreen rectangle (which
/// caused periodic lag spikes). A click/keystroke is one tiny rect; a full
/// repaint splits into ~`h / BAND_H` rects.
fn compute_regions(image: &DecodedImage, last_frame: &[u8]) -> Option<Vec<RegionJob>> {
    let w = image.width() as usize;
    let h = image.height() as usize;
    let data = image.data();

    // First frame / resize: whole screen in one rect.
    if last_frame.len() != data.len() {
        return Some(vec![make_region(data, w, 0, 0, w, h)]);
    }

    let mut jobs = Vec::new();
    let mut by = 0usize;
    while by < h {
        let bh = BAND_H.min(h - by);
        if let Some((rx, ry, rw, rh)) = changed_band(data, last_frame, w, by, bh) {
            jobs.push(make_region(data, w, rx, ry, rw, rh));
        }
        by += bh;
    }

    if jobs.is_empty() {
        None
    } else {
        Some(jobs)
    }
}

/// Extract one rectangle's RGB pixels into an owned `RegionJob`.
fn make_region(data: &[u8], full_w: usize, x: usize, y: usize, w: usize, h: usize) -> RegionJob {
    RegionJob {
        x: x as u16,
        y: y as u16,
        w: w as u16,
        h: h as u16,
        rgb: extract_rgb(data, full_w, x, y, w, h),
    }
}

/// Bounding box of pixels that differ from `last` **within rows
/// `[y0, y0+bh)`**. `None` if the band is unchanged.
fn changed_band(
    cur: &[u8],
    last: &[u8],
    w: usize,
    y0: usize,
    bh: usize,
) -> Option<(usize, usize, usize, usize)> {
    let stride = w * 4;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, y0 + bh, 0usize, 0usize);
    let mut changed = false;
    for y in y0..y0 + bh {
        let row_cur = &cur[y * stride..(y + 1) * stride];
        let row_last = &last[y * stride..(y + 1) * stride];
        if row_cur == row_last {
            continue;
        }
        changed = true;
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
        // Narrow the changed column range within this row.
        let mut l = 0usize;
        while l < w && row_cur[l * 4..l * 4 + 4] == row_last[l * 4..l * 4 + 4] {
            l += 1;
        }
        if l < min_x {
            min_x = l;
        }
        let mut r = w;
        while r > l && row_cur[(r - 1) * 4..r * 4] == row_last[(r - 1) * 4..r * 4] {
            r -= 1;
        }
        if r > max_x {
            max_x = r;
        }
    }
    if !changed {
        return None;
    }
    Some((min_x, min_y, max_x - min_x, max_y - min_y + 1))
}

/// Copy a sub-rectangle of an RGBA framebuffer into a tight RGB buffer
/// (JPEG has no alpha).
fn extract_rgb(rgba: &[u8], full_w: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let stride = full_w * 4;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in y..y + h {
        let base = row * stride + x * 4;
        for col in 0..w {
            let p = base + col * 4;
            rgb.push(rgba[p]);
            rgb.push(rgba[p + 1]);
            rgb.push(rgba[p + 2]);
        }
    }
    rgb
}

/// RGB buffer → JPEG → base64.
fn encode_jpeg_base64(rgb: &[u8], width: u16, height: u16) -> Option<String> {
    use base64::Engine as _;
    let mut jpeg = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY);
    encoder
        .encode(rgb, u32::from(width), u32::from(height), image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&jpeg))
}

/// RGB buffer → PNG → base64. Lossless: text/UI stay crisp (no JPEG ringing).
fn encode_png_base64(rgb: &[u8], width: u32, height: u32) -> Option<String> {
    use base64::Engine as _;
    use image::ImageEncoder as _;
    let mut png = Vec::new();
    // Default encoder uses adaptive filtering + balanced compression — fine
    // for the small regions we send, and keeps the output lossless.
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgb, width, height, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&png))
}

/// Encode a UI input event into fast-path input PDUs and write them to the
/// server. Mouse is fully wired (validated by the spike); keyboard +
/// modifier-sync land in the next slice (2b-2b) once the scancode mapping
/// is in place — for now those events are accepted and ignored.
fn send_input(
    active_stage: &mut ActiveStage,
    input_db: &mut Database,
    framed: &mut UpgradedFramed,
    image: &mut DecodedImage,
    ev: RdpInputEvent,
) -> std::io::Result<()> {
    let ops: Vec<Operation> = match ev {
        RdpInputEvent::MouseMove { x, y } => vec![Operation::MouseMove(MousePosition { x, y })],
        RdpInputEvent::MouseButton {
            button,
            pressed,
            x,
            y,
        } => {
            info!(?button, pressed, x, y, "rdp mouse button");
            let b = map_button(button);
            vec![
                Operation::MouseMove(MousePosition { x, y }),
                if pressed {
                    Operation::MouseButtonPressed(b)
                } else {
                    Operation::MouseButtonReleased(b)
                },
            ]
        }
        RdpInputEvent::MouseWheel { delta, .. } => {
            vec![Operation::WheelRotations(WheelRotations {
                is_vertical: true,
                rotation_units: delta,
            })]
        }
        // Keyboard + modifier-sync: next slice (2b-2b).
        RdpInputEvent::Key { .. }
        | RdpInputEvent::SyncModifiers { .. }
        | RdpInputEvent::ReleaseAllModifiers => return Ok(()),
    };

    let fastpath = input_db.apply(ops);
    let outputs = active_stage
        .process_fastpath_input(image, &fastpath)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    for out in outputs {
        if let ActiveStageOutput::ResponseFrame(frame) = out {
            framed.write_all(&frame)?;
        }
    }
    Ok(())
}

fn map_button(b: MouseButton) -> IrMouseButton {
    match b {
        MouseButton::Left => IrMouseButton::Left,
        MouseButton::Middle => IrMouseButton::Middle,
        MouseButton::Right => IrMouseButton::Right,
    }
}

// =====================================================================
// IronRDP connect — ported from the validated spike, adapted to typed
// `RdpError` and returning a socket handle for read-timeout control.
// =====================================================================

type UpgradedFramed =
    ironrdp_blocking::Framed<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>;

fn build_config(
    username: String,
    domain: Option<String>,
    password: String,
    options: &crate::RdpOpenOptions,
) -> connector::Config {
    connector::Config {
        credentials: Credentials::UsernamePassword { username, password },
        domain,
        enable_tls: false, // we drive the TLS upgrade ourselves
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: options.keyboard_layout,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: options.width,
            height: options.height,
        },
        bitmap: None,
        client_build: 0,
        client_name: "RemoteHub".to_owned(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: MajorPlatformType::WINDOWS,
        enable_server_pointer: false,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        pointer_software_rendering: true,
        // Disable nothing → the server shows the full experience: full window
        // contents while dragging (not just an outline), wallpaper, themes,
        // menu animations. IronRDP's `default()` disables full-window-drag
        // (and other bits) for bandwidth; we want the mstsc-on-LAN feel.
        // Trade-off: dragging streams the whole moving window, so it leans on
        // our encode/transport — the worst-case for the lag we're profiling.
        performance_flags: PerformanceFlags::empty(),
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
    }
}

fn connect(
    config: connector::Config,
    server_name: String,
    port: u16,
    events: &mpsc::UnboundedSender<RdpSessionEvent>,
) -> Result<(ConnectionResult, UpgradedFramed), RdpError> {
    let emit = |state: RdpState| {
        let _ = events.send(RdpSessionEvent::StateChanged { state });
    };

    use std::net::ToSocketAddrs as _;
    emit(RdpState::Resolving);
    let server_addr = (server_name.as_str(), port)
        .to_socket_addrs()
        .map_err(RdpError::Network)?
        .next()
        .ok_or_else(|| RdpError::Connector("could not resolve host".into()))?;

    // Reaching the host: TCP connect, RDP negotiation, TLS upgrade. If the
    // machine is off/unreachable this is where we block until timeout — and
    // the UI correctly shows "Connecting", not "Authenticating".
    emit(RdpState::Connecting);
    let tcp_stream = TcpStream::connect(server_addr).map_err(RdpError::Network)?;
    // Generous timeout for the (multi-roundtrip) handshake. Swapped to a
    // short poll timeout on the *same* socket after connect — see below.
    tcp_stream
        .set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(RdpError::Network)?;
    let client_addr = tcp_stream.local_addr().map_err(RdpError::Network)?;

    let mut framed = ironrdp_blocking::Framed::new(tcp_stream);
    let mut connector = connector::ClientConnector::new(config, client_addr);

    let should_upgrade = ironrdp_blocking::connect_begin(&mut framed, &mut connector)
        .map_err(|e| RdpError::Connector(e.to_string()))?;

    let initial_stream = framed.into_inner_no_leftover();
    let (upgraded_stream, server_public_key) = tls_upgrade(initial_stream, server_name.clone())?;

    let upgraded = ironrdp_blocking::mark_as_upgraded(should_upgrade, &mut connector);
    let mut upgraded_framed = ironrdp_blocking::Framed::new(upgraded_stream);
    let mut network_client = ReqwestNetworkClient;

    // Credential exchange (CredSSP/NTLM). A wrong password fails here.
    emit(RdpState::Authenticating);
    let connection_result = ironrdp_blocking::connect_finalize(
        upgraded,
        connector,
        &mut upgraded_framed,
        &mut network_client,
        server_name.into(),
        server_public_key,
        None,
    )
    .map_err(|e| RdpError::Connector(e.to_string()))?;

    // CRITICAL: switch the *actual* socket the active loop reads from to a
    // short poll timeout, so the worker drains UI input every ~16ms instead
    // of blocking in read_pdu until the next server frame. Must be set on
    // this socket — a timeout on a try_clone()'d handle does NOT apply to
    // reads here on Windows (that bug caused multi-second input lag).
    let _ = upgraded_framed
        .get_inner_mut()
        .0
        .sock
        .set_read_timeout(Some(READ_POLL));

    Ok((connection_result, upgraded_framed))
}

fn tls_upgrade(
    stream: TcpStream,
    server_name: String,
) -> Result<(rustls::StreamOwned<rustls::ClientConnection, TcpStream>, Vec<u8>), RdpError> {
    // TOFU at the RDP layer (RdpCertStore) is wired in a later pass; for now
    // we accept the cert at the TLS layer and rely on CredSSP for identity,
    // matching the validated spike.
    let mut config = rustls::client::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(danger::NoCertificateVerification))
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    let config = Arc::new(config);

    let server_name = server_name
        .try_into()
        .map_err(|_| RdpError::Tls("invalid server name".into()))?;
    let client = rustls::ClientConnection::new(config, server_name)
        .map_err(|e| RdpError::Tls(e.to_string()))?;
    let mut tls_stream = rustls::StreamOwned::new(client, stream);
    tls_stream.flush().map_err(RdpError::Network)?;

    let cert = tls_stream
        .conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or_else(|| RdpError::Tls("peer certificate missing".into()))?;
    let server_public_key = extract_tls_server_public_key(cert)?;
    Ok((tls_stream, server_public_key))
}

fn extract_tls_server_public_key(cert: &[u8]) -> Result<Vec<u8>, RdpError> {
    use x509_cert::der::Decode as _;
    let cert = x509_cert::Certificate::from_der(cert).map_err(|e| RdpError::Tls(e.to_string()))?;
    let key = cert
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| RdpError::Tls("subject public key not byte-aligned".into()))?
        .to_owned();
    Ok(key)
}

/// `RdpError` isn't `Clone` (it wraps `io::Error`); derive the close reason
/// before we consume it for the error message.
fn close_reason_for(e: &RdpError) -> RdpCloseReason {
    match e {
        RdpError::AuthFailed => RdpCloseReason::AuthFailed,
        RdpError::CertUntrusted | RdpError::CertRejected => RdpCloseReason::CertRejected,
        _ => RdpCloseReason::Error,
    }
}

mod danger {
    use tokio_rustls::rustls::client::danger::{
        HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
    };
    use tokio_rustls::rustls::{pki_types, DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct NoCertificateVerification;

    impl ServerCertVerifier for NoCertificateVerification {
        fn verify_server_cert(
            &self,
            _: &pki_types::CertificateDer<'_>,
            _: &[pki_types::CertificateDer<'_>],
            _: &pki_types::ServerName<'_>,
            _: &[u8],
            _: pki_types::UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &pki_types::CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &pki_types::CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![
                SignatureScheme::RSA_PKCS1_SHA1,
                SignatureScheme::ECDSA_SHA1_Legacy,
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::RSA_PKCS1_SHA384,
                SignatureScheme::ECDSA_NISTP384_SHA384,
                SignatureScheme::RSA_PKCS1_SHA512,
                SignatureScheme::ECDSA_NISTP521_SHA512,
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::RSA_PSS_SHA384,
                SignatureScheme::RSA_PSS_SHA512,
                SignatureScheme::ED25519,
                SignatureScheme::ED448,
            ]
        }
    }
}
