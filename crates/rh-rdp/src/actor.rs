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
//! Round 2b-2 adds input: mouse, keyboard, and modifier-sync, all driven
//! through the IronRDP `input` `Database` + fast-path PDUs (see
//! `send_input` / `code_to_scancode`). Keyboard uses a PS/2 Set 1 scancode
//! map; modifier-sync (release-all on blur, diff-resync on focus) is the
//! anti-stuck-modifier fix that motivated owning the input path.
//!
//! Frame strategy (MVP): push the whole framebuffer, throttled to ~10 fps.
//! Region-diffing + a faster transport than JSON are the documented
//! follow-up (see `docs/specs/rdp-session.md`, Open-Q #1).

use std::io::Write as _;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use ironrdp::cliprdr::backend::CliprdrBackend;
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use ironrdp::cliprdr::CliprdrClient;
use ironrdp::core::{impl_as_any, IntoOwned};
use ironrdp::displaycontrol::client::DisplayControlClient;
use ironrdp::dvc::DrdynvcClient;
use ironrdp::connector::connection_activation::{
    ConnectionActivationSequence, ConnectionActivationState,
};
use ironrdp::connector::{self, ConnectionResult, Credentials, Sequence as _, State as _};
use ironrdp::core::WriteBuf;
use ironrdp::session::fast_path::ProcessorBuilder;
use ironrdp::input::{
    Database, MouseButton as IrMouseButton, MousePosition, Operation, Scancode, WheelRotations,
};
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
    // Local clipboard text pushed by the UI (client→server paste).
    let (clip_tx, clip_rx) = std::sync::mpsc::channel::<LocalClipUpdate>();
    // Viewport size changes (px) from the UI → DisplayControl resize.
    let (resize_tx, resize_rx) = std::sync::mpsc::channel::<(u16, u16)>();

    let worker = match std::thread::Builder::new()
        .name("rdp-session".to_owned())
        .spawn(move || blocking_session(params, &worker_shutdown, &worker_events, &input_rx, &clip_rx, &resize_rx))
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
            Some(RdpCommand::SetClipboard(text)) => {
                let _ = clip_tx.send(LocalClipUpdate::Text(text));
            }
            Some(RdpCommand::SetClipboardImage {
                width,
                height,
                rgba,
            }) => {
                let _ = clip_tx.send(LocalClipUpdate::Image(LocalImage {
                    width,
                    height,
                    rgba,
                }));
            }
            Some(RdpCommand::Resize { width, height }) => {
                let _ = resize_tx.send((width, height));
            }
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
    clip_rx: &std::sync::mpsc::Receiver<LocalClipUpdate>,
    resize_rx: &std::sync::mpsc::Receiver<(u16, u16)>,
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

    // CLIPRDR (clipboard) plumbing. The backend (registered as a static
    // channel on the connector) fires callbacks from inside
    // `active_stage.process`; it can't touch the active stage itself, so it
    // posts `ClipMsg`s that the loop drains and turns into channel PDUs.
    // `local_clip` is the text the UI offered for client→server paste.
    let (clip_msg_tx, clip_msg_rx) = std::sync::mpsc::channel::<ClipMsg>();
    let local_clip: Arc<Mutex<LocalClip>> = Arc::new(Mutex::new(LocalClip::default()));
    let backend: Box<dyn CliprdrBackend> = Box::new(ClipboardBridge {
        tx: clip_msg_tx.clone(),
        events: events.clone(),
        local: Arc::clone(&local_clip),
        temp_dir: String::from("."),
        pending_format: None,
    });

    // connect() emits the real phase transitions (Resolving → Connecting →
    // Authenticating) as it progresses, so the UI reflects what's actually
    // happening instead of claiming "Authenticating" before we've even
    // reached the host.
    let (connection_result, mut framed) =
        match connect(config, params.host.hostname.clone(), params.host.port, events, backend) {
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
    // Latest viewport size the UI asked for. `encode_resize` returns None
    // until the DisplayControl DVC has received caps, so we keep retrying
    // each tick until it lands (then clear it).
    let mut pending_resize: Option<(u16, u16)> = None;

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

        // Local clipboard updates from the UI: store text/image and advertise
        // the matching formats (the server requests the data on paste).
        while let Ok(update) = clip_rx.try_recv() {
            let formats = if let Ok(mut g) = local_clip.lock() {
                match update {
                    LocalClipUpdate::Text(text) => {
                        g.text = Some(text);
                        g.image = None;
                    }
                    LocalClipUpdate::Image(img) => {
                        g.image = Some(img);
                        g.text = None;
                    }
                }
                let mut fmts = Vec::new();
                if g.text.is_some() {
                    fmts.push(ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT));
                }
                if g.image.is_some() {
                    fmts.push(ClipboardFormat::new(ClipboardFormatId::CF_DIB));
                }
                fmts
            } else {
                Vec::new()
            };
            if !formats.is_empty() {
                let _ = clip_msg_tx.send(ClipMsg::Copy(formats));
            }
        }

        // Coalesce viewport resizes (only the latest size matters), then try
        // to apply via the Display Control DVC. If the channel isn't ready
        // yet, keep the size pending and retry next tick.
        while let Ok(size) = resize_rx.try_recv() {
            pending_resize = Some(size);
        }
        if let Some((w, h)) = pending_resize {
            // Spec: 200..=8192, width must be even.
            let w = (u32::from(w).clamp(200, 8192)) & !1;
            let h = u32::from(h).clamp(200, 8192);
            match active_stage.encode_resize(w, h, None, None) {
                Some(Ok(frame)) => {
                    pending_resize = None;
                    if framed.write_all(&frame).is_err() {
                        emit(RdpSessionEvent::Closed {
                            reason: RdpCloseReason::ServerDisconnected { code: None },
                        });
                        return;
                    }
                }
                Some(Err(e)) => {
                    pending_resize = None;
                    warn!("displaycontrol resize failed: {e}");
                }
                None => { /* DVC not ready yet — retry next tick */ }
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
                ActiveStageOutput::DeactivateAll(mut activation) => {
                    // A resolution change (our DisplayControl resize, or a
                    // server-driven mode change) triggers a Deactivate-All →
                    // reactivation handshake. Drive it to completion, then
                    // rebuild the framebuffer + fast-path processor at the new
                    // size. The active loop runs on a 16ms read timeout; the
                    // handshake needs longer, so widen it for the duration.
                    let _ = framed
                        .get_inner_mut()
                        .0
                        .sock
                        .set_read_timeout(Some(CONNECT_TIMEOUT));
                    let result = run_reactivation(&mut framed, &mut *activation);
                    let _ = framed
                        .get_inner_mut()
                        .0
                        .sock
                        .set_read_timeout(Some(READ_POLL));
                    match result {
                        Ok(Some((processor, w, h))) => {
                            active_stage.set_fastpath_processor(processor);
                            image = DecodedImage::new(
                                ironrdp::graphics::image_processing::PixelFormat::RgbA32,
                                w,
                                h,
                            );
                            last_frame = Vec::new();
                            last_emit = Instant::now()
                                .checked_sub(FRAME_INTERVAL)
                                .unwrap_or_else(Instant::now);
                            emit(RdpSessionEvent::Resized {
                                width: w,
                                height: h,
                            });
                            debug!(width = w, height = h, "rdp reactivated");
                        }
                        Ok(None) => warn!("reactivation finished without a finalized state"),
                        Err(e) => {
                            emit(RdpSessionEvent::Error {
                                message: format!("reactivation: {e}"),
                            });
                            emit(RdpSessionEvent::Closed {
                                reason: RdpCloseReason::Error,
                            });
                            return;
                        }
                    }
                }
                ActiveStageOutput::PointerBitmap(ptr) => {
                    use base64::Engine as _;
                    emit(RdpSessionEvent::PointerBitmap {
                        width: ptr.width,
                        height: ptr.height,
                        hotspot_x: ptr.hotspot_x,
                        hotspot_y: ptr.hotspot_y,
                        rgba_base64: base64::engine::general_purpose::STANDARD
                            .encode(&ptr.bitmap_data),
                    });
                }
                ActiveStageOutput::PointerHidden => {
                    emit(RdpSessionEvent::PointerHidden);
                }
                ActiveStageOutput::PointerDefault => {
                    emit(RdpSessionEvent::PointerDefault);
                }
                _ => {}
            }
        }

        // Send any pending CLIPRDR channel PDUs produced by the clipboard
        // backend (during process() above) or by a local clipboard update.
        while let Ok(msg) = clip_msg_rx.try_recv() {
            let produced = {
                let Some(cliprdr) = active_stage.get_svc_processor::<CliprdrClient>() else {
                    continue;
                };
                match msg {
                    ClipMsg::Copy(formats) => cliprdr.initiate_copy(&formats),
                    ClipMsg::Paste(format) => cliprdr.initiate_paste(format),
                    ClipMsg::Data(response) => cliprdr.submit_format_data(response),
                }
            };
            match produced {
                Ok(svc_messages) => {
                    match active_stage.process_svc_processor_messages(svc_messages) {
                        Ok(frame) => {
                            if framed.write_all(&frame).is_err() {
                                emit(RdpSessionEvent::Closed {
                                    reason: RdpCloseReason::ServerDisconnected { code: None },
                                });
                                return;
                            }
                        }
                        Err(e) => warn!("cliprdr encode failed: {e}"),
                    }
                }
                Err(e) => warn!("cliprdr op failed: {e}"),
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
/// server. Mouse, keyboard and modifier-sync are all wired.
///
/// Keyboard: the UI sends a browser `KeyboardEvent.code` string; we map it
/// to a PS/2 Set 1 scancode (`code_to_scancode`) and feed the `Database`,
/// which turns press/release into the right fast-path keyboard PDUs (and
/// synthesises the release-before-press on auto-repeat).
///
/// Modifier-sync (the reason we own the input path — fixes stuck Ctrl/Alt
/// after Alt-Tab): on focus loss the UI sends `ReleaseAllModifiers` and we
/// `Database::release_all()` everything the server believes is held; on
/// focus gain it sends `SyncModifiers` with the OS modifier state and we
/// diff it against the `Database` and emit only the deltas.
fn send_input(
    active_stage: &mut ActiveStage,
    input_db: &mut Database,
    framed: &mut UpgradedFramed,
    image: &mut DecodedImage,
    ev: RdpInputEvent,
) -> std::io::Result<()> {
    let fastpath = match ev {
        RdpInputEvent::MouseMove { x, y } => {
            input_db.apply([Operation::MouseMove(MousePosition { x, y })])
        }
        RdpInputEvent::MouseButton {
            button,
            pressed,
            x,
            y,
        } => {
            info!(?button, pressed, x, y, "rdp mouse button");
            let b = map_button(button);
            input_db.apply([
                Operation::MouseMove(MousePosition { x, y }),
                if pressed {
                    Operation::MouseButtonPressed(b)
                } else {
                    Operation::MouseButtonReleased(b)
                },
            ])
        }
        RdpInputEvent::MouseWheel { delta, .. } => input_db.apply([Operation::WheelRotations(
            WheelRotations {
                is_vertical: true,
                rotation_units: delta,
            },
        )]),
        RdpInputEvent::Key { code, pressed, .. } => match code_to_scancode(&code) {
            Some((extended, c)) => {
                let sc = Scancode::from_u8(extended, c);
                input_db.apply([if pressed {
                    Operation::KeyPressed(sc)
                } else {
                    Operation::KeyReleased(sc)
                }])
            }
            None => {
                debug!(%code, "rdp: unmapped key code, ignoring");
                return Ok(());
            }
        },
        RdpInputEvent::RawScancode {
            scancode,
            extended,
            pressed,
        } => {
            let sc = Scancode::from_u8(extended, scancode);
            input_db.apply([if pressed {
                Operation::KeyPressed(sc)
            } else {
                Operation::KeyReleased(sc)
            }])
        }
        // Focus gained: re-press / release modifiers so the server's view
        // matches the physical keyboard. Diff against the Database so we
        // only emit changes (a redundant KeyPressed would fire a spurious
        // auto-repeat). Lock-LED sync (Caps/Num/Scroll) needs a
        // TS_SYNC_EVENT, not exposed by ironrdp-input's fast-path Database
        // — deferred; the stuck-modifier fix below is the headline.
        RdpInputEvent::SyncModifiers {
            ctrl, alt, shift, meta, ..
        } => {
            let mut ops: Vec<Operation> = Vec::new();
            sync_mod(input_db, &mut ops, false, 0x1D, ctrl); // Ctrl (left)
            sync_mod(input_db, &mut ops, false, 0x38, alt); // Alt (left)
            sync_mod(input_db, &mut ops, false, 0x2A, shift); // Shift (left)
            sync_mod(input_db, &mut ops, true, 0x5B, meta); // Meta/Win (left, extended)
            if ops.is_empty() {
                return Ok(());
            }
            input_db.apply(ops)
        }
        // Focus lost: blanket-release everything held. This is the actual
        // cure for the classic "Ctrl/Alt stuck down after Alt-Tab" bug.
        RdpInputEvent::ReleaseAllModifiers => input_db.release_all(),
    };

    if fastpath.is_empty() {
        return Ok(());
    }
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

/// Push the press/release needed to make modifier `(extended, code)` match
/// `desired`, given what the `Database` currently believes is held. No-op
/// when already in the desired state.
fn sync_mod(db: &Database, ops: &mut Vec<Operation>, extended: bool, code: u8, desired: bool) {
    let sc = Scancode::from_u8(extended, code);
    let held = db.is_key_pressed(sc);
    if desired && !held {
        ops.push(Operation::KeyPressed(sc));
    } else if !desired && held {
        ops.push(Operation::KeyReleased(sc));
    }
}

/// Map a W3C `KeyboardEvent.code` to a PS/2 **Set 1** scancode as
/// `(extended, code)`. `extended` marks the keys the PS/2 wire protocol
/// prefixes with `0xE0` (right-hand modifiers, the nav cluster, arrows,
/// numpad Enter/`/`, the Windows/Menu keys). Returns `None` for codes we
/// don't forward (caller ignores them).
///
/// Note the deliberate code reuse between the navigation cluster (extended)
/// and the numeric keypad (non-extended): e.g. `Home` is `(true, 0x47)`
/// while `Numpad7` is `(false, 0x47)`. The `Database` keys its held-state
/// table on `(extended, code)`, so they never collide.
fn code_to_scancode(code: &str) -> Option<(bool, u8)> {
    Some(match code {
        // Letters
        "KeyA" => (false, 0x1E),
        "KeyB" => (false, 0x30),
        "KeyC" => (false, 0x2E),
        "KeyD" => (false, 0x20),
        "KeyE" => (false, 0x12),
        "KeyF" => (false, 0x21),
        "KeyG" => (false, 0x22),
        "KeyH" => (false, 0x23),
        "KeyI" => (false, 0x17),
        "KeyJ" => (false, 0x24),
        "KeyK" => (false, 0x25),
        "KeyL" => (false, 0x26),
        "KeyM" => (false, 0x32),
        "KeyN" => (false, 0x31),
        "KeyO" => (false, 0x18),
        "KeyP" => (false, 0x19),
        "KeyQ" => (false, 0x10),
        "KeyR" => (false, 0x13),
        "KeyS" => (false, 0x1F),
        "KeyT" => (false, 0x14),
        "KeyU" => (false, 0x16),
        "KeyV" => (false, 0x2F),
        "KeyW" => (false, 0x11),
        "KeyX" => (false, 0x2D),
        "KeyY" => (false, 0x15),
        "KeyZ" => (false, 0x2C),
        // Number row
        "Digit1" => (false, 0x02),
        "Digit2" => (false, 0x03),
        "Digit3" => (false, 0x04),
        "Digit4" => (false, 0x05),
        "Digit5" => (false, 0x06),
        "Digit6" => (false, 0x07),
        "Digit7" => (false, 0x08),
        "Digit8" => (false, 0x09),
        "Digit9" => (false, 0x0A),
        "Digit0" => (false, 0x0B),
        // Punctuation / whitespace
        "Minus" => (false, 0x0C),
        "Equal" => (false, 0x0D),
        "Backspace" => (false, 0x0E),
        "Tab" => (false, 0x0F),
        "BracketLeft" => (false, 0x1A),
        "BracketRight" => (false, 0x1B),
        "Enter" => (false, 0x1C),
        "Semicolon" => (false, 0x27),
        "Quote" => (false, 0x28),
        "Backquote" => (false, 0x29),
        "Backslash" => (false, 0x2B),
        "Comma" => (false, 0x33),
        "Period" => (false, 0x34),
        "Slash" => (false, 0x35),
        "Space" => (false, 0x39),
        "IntlBackslash" => (false, 0x56), // ISO key by the left Shift
        "IntlRo" => (false, 0x73),        // JP/BR  \_  key
        "IntlYen" => (false, 0x7D),       // JP  ¥|  key
        // Modifiers
        "ControlLeft" => (false, 0x1D),
        "ControlRight" => (true, 0x1D),
        "ShiftLeft" => (false, 0x2A),
        "ShiftRight" => (false, 0x36),
        "AltLeft" => (false, 0x38),
        "AltRight" => (true, 0x38), // AltGr
        "MetaLeft" => (true, 0x5B),
        "MetaRight" => (true, 0x5C),
        "ContextMenu" => (true, 0x5D),
        "CapsLock" => (false, 0x3A),
        "NumLock" => (false, 0x45),
        "ScrollLock" => (false, 0x46),
        // Function row
        "Escape" => (false, 0x01),
        "F1" => (false, 0x3B),
        "F2" => (false, 0x3C),
        "F3" => (false, 0x3D),
        "F4" => (false, 0x3E),
        "F5" => (false, 0x3F),
        "F6" => (false, 0x40),
        "F7" => (false, 0x41),
        "F8" => (false, 0x42),
        "F9" => (false, 0x43),
        "F10" => (false, 0x44),
        "F11" => (false, 0x57),
        "F12" => (false, 0x58),
        // Navigation cluster (extended)
        "Insert" => (true, 0x52),
        "Delete" => (true, 0x53),
        "Home" => (true, 0x47),
        "End" => (true, 0x4F),
        "PageUp" => (true, 0x49),
        "PageDown" => (true, 0x51),
        "ArrowUp" => (true, 0x48),
        "ArrowLeft" => (true, 0x4B),
        "ArrowRight" => (true, 0x4D),
        "ArrowDown" => (true, 0x50),
        // Numeric keypad
        "Numpad0" => (false, 0x52),
        "Numpad1" => (false, 0x4F),
        "Numpad2" => (false, 0x50),
        "Numpad3" => (false, 0x51),
        "Numpad4" => (false, 0x4B),
        "Numpad5" => (false, 0x4C),
        "Numpad6" => (false, 0x4D),
        "Numpad7" => (false, 0x47),
        "Numpad8" => (false, 0x48),
        "Numpad9" => (false, 0x49),
        "NumpadDecimal" => (false, 0x53),
        "NumpadAdd" => (false, 0x4E),
        "NumpadSubtract" => (false, 0x4A),
        "NumpadMultiply" => (false, 0x37),
        "NumpadDivide" => (true, 0x35),
        "NumpadEnter" => (true, 0x1C),
        // Best-effort: full PrintScreen is E0 2A E0 37; send the meaningful half.
        "PrintScreen" => (true, 0x37),
        _ => return None,
    })
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

/// Drive a `DeactivateAll` reactivation sequence to completion over the
/// blocking framed stream, returning a fresh fast-path processor + the new
/// desktop size. Mirrors the connector's single-step loop, generic over the
/// `Sequence` trait (the blocking helper is hard-coded to `ClientConnector`).
fn run_reactivation(
    framed: &mut UpgradedFramed,
    seq: &mut ConnectionActivationSequence,
) -> Result<Option<(ironrdp::session::fast_path::Processor, u16, u16)>, RdpError> {
    let mut buf = WriteBuf::new();
    loop {
        if seq.state().is_terminal() {
            return Ok(match seq.connection_activation_state() {
                ConnectionActivationState::Finalized {
                    io_channel_id,
                    user_channel_id,
                    desktop_size,
                    enable_server_pointer,
                    pointer_software_rendering,
                } => {
                    let processor = ProcessorBuilder {
                        io_channel_id,
                        user_channel_id,
                        enable_server_pointer,
                        pointer_software_rendering,
                    }
                    .build();
                    Some((processor, desktop_size.width, desktop_size.height))
                }
                _ => None,
            });
        }

        buf.clear();
        let written = if let Some(hint) = seq.next_pdu_hint() {
            let pdu = framed.read_by_hint(hint).map_err(RdpError::Network)?;
            seq.step(pdu.as_ref(), &mut buf)
                .map_err(|e| RdpError::Connector(e.to_string()))?
        } else {
            seq.step_no_input(&mut buf)
                .map_err(|e| RdpError::Connector(e.to_string()))?
        };
        if let Some(len) = written.size() {
            framed.write_all(&buf[..len]).map_err(RdpError::Network)?;
        }
    }
}

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
        enable_server_pointer: true,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        // false → IronRDP emits `PointerBitmap` updates (non-premultiplied
        // RGBA) instead of compositing the cursor into the framebuffer. We
        // render it client-side as a CSS cursor so it tracks the local mouse
        // instantly rather than lagging the server's repaint rate.
        pointer_software_rendering: false,
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
    cliprdr_backend: Box<dyn CliprdrBackend>,
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
    let mut connector = connector::ClientConnector::new(config, client_addr)
        .with_static_channel(CliprdrClient::new(cliprdr_backend))
        // DRDYNVC hosting the Display Control DVC → dynamic resize. The
        // callback fires when the server sends caps (channel becomes ready);
        // we don't need to respond, so return no messages.
        .with_static_channel(
            DrdynvcClient::new()
                .with_dynamic_channel(DisplayControlClient::new(|_caps| Ok(Vec::new()))),
        );

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

// =====================================================================
// CLIPRDR (clipboard) — client backend bridging the channel to the UI.
//
// Text only (CF_UNICODETEXT). The OS clipboard itself lives in the
// frontend (Tauri); this backend only shuttles text:
//   - server→client (remote copy): when the remote offers text we request
//     it; the response is decoded and emitted to the UI, which writes it to
//     the OS clipboard.
//   - client→server (paste into remote): the UI pushes its clipboard text
//     (RdpCommand::SetClipboard); we advertise CF_UNICODETEXT and answer the
//     server's data request from `local`.
//
// The backend can't touch the ActiveStage (it runs inside its `process`),
// so it posts `ClipMsg`s the worker loop drains and encodes.
// =====================================================================

/// Outgoing CLIPRDR action the worker loop turns into channel PDUs.
enum ClipMsg {
    /// Advertise the given formats to the server (we have data to offer).
    Copy(Vec<ClipboardFormat>),
    /// Request the given format's data from the server.
    Paste(ClipboardFormatId),
    /// Answer a server data request with this payload.
    Data(OwnedFormatDataResponse),
}

/// Local OS clipboard content offered to the remote (client→server paste).
/// Holds at most one of text/image — whatever the OS clipboard last had.
#[derive(Default, Debug)]
struct LocalClip {
    text: Option<String>,
    image: Option<LocalImage>,
}

/// A local clipboard image as top-down RGBA (from the UI's `Image.rgba()`).
#[derive(Clone, Debug)]
struct LocalImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Update pushed from the UI when the local OS clipboard changes.
enum LocalClipUpdate {
    Text(String),
    Image(LocalImage),
}

#[derive(Debug)]
struct ClipboardBridge {
    tx: std::sync::mpsc::Sender<ClipMsg>,
    events: mpsc::UnboundedSender<RdpSessionEvent>,
    local: Arc<Mutex<LocalClip>>,
    temp_dir: String,
    /// Format we last asked the server for (set in `on_remote_copy`, read in
    /// `on_format_data_response` — the response carries no format id).
    pending_format: Option<ClipboardFormatId>,
}

impl_as_any!(ClipboardBridge);

impl CliprdrBackend for ClipboardBridge {
    fn temporary_directory(&self) -> &str {
        &self.temp_dir
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        // Text + image (CF_DIB) only — no file-transfer capabilities.
        ClipboardGeneralCapabilityFlags::empty()
    }

    fn on_ready(&mut self) {}

    fn on_request_format_list(&mut self) {
        // Advertise whatever we currently hold (text and/or image).
        let formats = self
            .local
            .lock()
            .ok()
            .map(|g| {
                let mut f = Vec::new();
                if g.text.is_some() {
                    f.push(ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT));
                }
                if g.image.is_some() {
                    f.push(ClipboardFormat::new(ClipboardFormatId::CF_DIB));
                }
                f
            })
            .unwrap_or_default();
        let _ = self.tx.send(ClipMsg::Copy(formats));
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        _capabilities: ClipboardGeneralCapabilityFlags,
    ) {
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        // The remote put something on its clipboard. Prefer text; otherwise
        // pull an image (CF_DIB). Remember which we asked for — the data
        // response doesn't echo the format.
        let has_text = available_formats.iter().any(|f| {
            f.id == ClipboardFormatId::CF_UNICODETEXT || f.id == ClipboardFormatId::CF_TEXT
        });
        let has_image = available_formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_DIB || f.id == ClipboardFormatId::CF_DIBV5);
        if has_text {
            self.pending_format = Some(ClipboardFormatId::CF_UNICODETEXT);
            let _ = self.tx.send(ClipMsg::Paste(ClipboardFormatId::CF_UNICODETEXT));
        } else if has_image {
            self.pending_format = Some(ClipboardFormatId::CF_DIB);
            let _ = self.tx.send(ClipMsg::Paste(ClipboardFormatId::CF_DIB));
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        // Server wants our clipboard data (a paste happened in the remote).
        let response = if request.format == ClipboardFormatId::CF_UNICODETEXT {
            match self.local.lock().ok().and_then(|g| g.text.clone()) {
                Some(text) => FormatDataResponse::new_unicode_string(&text).into_owned(),
                None => FormatDataResponse::new_error().into_owned(),
            }
        } else if request.format == ClipboardFormatId::CF_DIB {
            let img = self.local.lock().ok().and_then(|g| g.image.clone());
            match img.and_then(|i| rgba_to_dib(&i.rgba, i.width, i.height)) {
                Some(dib) => FormatDataResponse::new_data(dib).into_owned(),
                None => FormatDataResponse::new_error().into_owned(),
            }
        } else {
            FormatDataResponse::new_error().into_owned()
        };
        let _ = self.tx.send(ClipMsg::Data(response));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        // Remote clipboard data arrived — hand it to the UI to put on the OS
        // clipboard. Decode per the format we requested.
        if response.is_error() {
            return;
        }
        match self.pending_format.take() {
            Some(fmt) if fmt == ClipboardFormatId::CF_DIB => {
                if let Some(png) = dib_to_png_base64(response.data()) {
                    let _ = self.events.send(RdpSessionEvent::Clipboard {
                        mime: "image/png".to_owned(),
                        data: png,
                    });
                }
            }
            _ => {
                if let Some(text) = decode_unicode_clipboard(response.data()) {
                    let _ = self.events.send(RdpSessionEvent::Clipboard {
                        mime: "text/plain".to_owned(),
                        data: text,
                    });
                }
            }
        }
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _data_id: LockDataId) {}
    fn on_unlock(&mut self, _data_id: LockDataId) {}
}

/// Decode a `CF_UNICODETEXT` clipboard payload (UTF-16LE, NUL-terminated).
fn decode_unicode_clipboard(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0) // stop at the NUL terminator
        .collect();
    String::from_utf16(&units).ok()
}

/// `CF_DIB` (BITMAPINFOHEADER + pixels, **no** file header) → PNG base64.
/// Output is opaque RGBA (DIB alpha is unreliable). Handles 24/32 bpp,
/// BI_RGB and BI_BITFIELDS, V3/V4/V5 headers, and bottom-up/top-down rows.
fn dib_to_png_base64(dib: &[u8]) -> Option<String> {
    if dib.len() < 40 {
        return None;
    }
    let rd32 = |o: usize| u32::from_le_bytes([dib[o], dib[o + 1], dib[o + 2], dib[o + 3]]);
    let bi_size = rd32(0) as usize;
    let width = rd32(4) as i32;
    let height_raw = rd32(8) as i32;
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = rd32(16);

    if bit_count != 24 && bit_count != 32 {
        return None; // palettized / exotic formats unsupported
    }
    let w = width.unsigned_abs() as usize;
    let h = height_raw.unsigned_abs() as usize;
    let bottom_up = height_raw > 0; // positive height = bottom-up rows
    if w == 0 || h == 0 || w > 10_000 || h > 10_000 {
        return None;
    }

    // Pixel data starts after the header (+ the 3-DWORD bitfield masks when a
    // V3 header uses BI_BITFIELDS; V4/V5 carry masks inside the header).
    let mut off = bi_size;
    if compression == 3 /* BI_BITFIELDS */ && bi_size == 40 {
        off += 12;
    }
    let bpp = (bit_count / 8) as usize;
    let row_stride = ((w * bpp + 3) / 4) * 4; // rows are 4-byte aligned
    if off.checked_add(row_stride.checked_mul(h)?)? > dib.len() {
        return None;
    }

    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_row = if bottom_up { h - 1 - y } else { y };
        let row = off + src_row * row_stride;
        for x in 0..w {
            let p = row + x * bpp; // DIB pixels are BGR(A)
            let d = (y * w + x) * 4;
            rgba[d] = dib[p + 2]; // R
            rgba[d + 1] = dib[p + 1]; // G
            rgba[d + 2] = dib[p]; // B
            rgba[d + 3] = 255; // opaque
        }
    }
    encode_rgba_png_base64(&rgba, w as u32, h as u32)
}

/// Top-down RGBA → `CF_DIB` (32-bpp BI_RGB, bottom-up BGRA). No file header.
fn rgba_to_dib(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return None;
    }
    let mut dib = Vec::with_capacity(40 + w * h * 4);
    // BITMAPINFOHEADER (V3, 40 bytes).
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(width as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&(height as i32).to_le_bytes()); // biHeight (+ = bottom-up)
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    // Pixels: bottom-up rows, BGRA. (32-bpp rows are already 4-byte aligned.)
    for y in 0..h {
        let src_row = h - 1 - y;
        for x in 0..w {
            let s = (src_row * w + x) * 4;
            dib.push(rgba[s + 2]); // B
            dib.push(rgba[s + 1]); // G
            dib.push(rgba[s]); // R
            dib.push(rgba[s + 3]); // A
        }
    }
    Some(dib)
}

/// RGBA buffer → PNG → base64 (lossless; for clipboard images).
fn encode_rgba_png_base64(rgba: &[u8], width: u32, height: u32) -> Option<String> {
    use base64::Engine as _;
    use image::ImageEncoder as _;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&png))
}
