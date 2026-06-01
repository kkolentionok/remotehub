//! RDP connectivity spike (Stage 4, round 2a).
//!
//! Validates the IronRDP connect + graphics pipeline against a REAL
//! Windows host in isolation, before any of it is wired into the session
//! actor or the app. It connects, decodes the first few seconds of
//! graphics, and writes the resulting desktop image to a PNG.
//!
//! Adapted nearly verbatim from IronRDP's official blocking example
//! (`crates/ironrdp/examples/screenshot.rs`, MIT/Apache-2.0,
//! © Devolutions). Kept faithful on purpose: this is the highest-
//! confidence reference for the exact 0.14 API surface. Once this runs
//! against your server, round 2b ports the validated connect into the
//! async `rh-rdp` actor and the rest of the app.
//!
//! # Run it
//! ```shell
//! cargo run -p rh-rdp --example rdp_spike -- \
//!     --host <IP> -u <USER> -p <PASS> [-d <DOMAIN>] -o shot.png
//! ```
//! It exits and saves the PNG after ~3s of no graphics activity (i.e.
//! once the desktop has settled). If `shot.png` shows your desktop, the
//! IronRDP path works end-to-end.

#![allow(clippy::print_stdout)]

use core::time::Duration;
use std::io::Write as _;
use std::net::TcpStream;
use std::path::PathBuf;

use anyhow::Context as _;
use ironrdp::connector::{self, ConnectionResult, Credentials};
use ironrdp::input::{Database, MouseButton as IrMouseButton, MousePosition, Operation};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageOutput};
use sspi::network_client::reqwest_network_client::ReqwestNetworkClient;
use tokio_rustls::rustls;
use tracing::{debug, info, trace};

fn main() -> anyhow::Result<()> {
    let action = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            println!("{HELP}");
            return Err(e.context("invalid argument(s)"));
        }
    };
    setup_logging()?;
    match action {
        Action::ShowHelp => {
            println!("{HELP}");
            Ok(())
        }
        Action::Run(cfg) => run(cfg),
    }
}

const HELP: &str = "\
USAGE:
  cargo run -p rh-rdp --example rdp_spike -- \\
      --host <HOST> [--port <PORT>] -u <USER> -p <PASS> \\
      [-d <DOMAIN>] [-o <OUTPUT.png>]
";

#[derive(Debug)]
struct RunConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
    output: PathBuf,
    domain: Option<String>,
}

enum Action {
    ShowHelp,
    Run(RunConfig),
}

fn parse_args() -> anyhow::Result<Action> {
    let mut args = pico_args::Arguments::from_env();
    if args.contains(["-h", "--help"]) {
        return Ok(Action::ShowHelp);
    }
    let host = args.value_from_str("--host")?;
    let port = args.opt_value_from_str("--port")?.unwrap_or(3389);
    let username = args.value_from_str(["-u", "--username"])?;
    let password = args.value_from_str(["-p", "--password"])?;
    let output = args
        .opt_value_from_str(["-o", "--output"])?
        .unwrap_or_else(|| PathBuf::from("shot.png"));
    let domain = args.opt_value_from_str(["-d", "--domain"])?;
    Ok(Action::Run(RunConfig {
        host,
        port,
        username,
        password,
        output,
        domain,
    }))
}

fn setup_logging() -> anyhow::Result<()> {
    use tracing::metadata::LevelFilter;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    let fmt_layer = tracing_subscriber::fmt::layer().compact();
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .with_env_var("IRONRDP_LOG")
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(env_filter)
        .try_init()
        .context("failed to set tracing subscriber")?;
    Ok(())
}

fn run(config: RunConfig) -> anyhow::Result<()> {
    let connector_config = build_config(config.username, config.password, config.domain)?;
    let (connection_result, framed) =
        connect(connector_config, config.host, config.port).context("connect")?;
    info!(
        width = connection_result.desktop_size.width,
        height = connection_result.desktop_size.height,
        "connected; capturing until idle"
    );

    let mut image = DecodedImage::new(
        ironrdp::graphics::image_processing::PixelFormat::RgbA32,
        connection_result.desktop_size.width,
        connection_result.desktop_size.height,
    );

    active_stage(connection_result, framed, &mut image).context("active stage")?;

    let img: image::ImageBuffer<image::Rgba<u8>, _> = image::ImageBuffer::from_raw(
        u32::from(image.width()),
        u32::from(image.height()),
        image.data().to_vec(),
    )
    .context("invalid image buffer")?;
    img.save(&config.output).context("save png")?;
    println!("Saved {}", config.output.display());
    Ok(())
}

fn build_config(
    username: String,
    password: String,
    domain: Option<String>,
) -> anyhow::Result<connector::Config> {
    Ok(connector::Config {
        credentials: Credentials::UsernamePassword { username, password },
        domain,
        // We drive the TLS upgrade ourselves below.
        enable_tls: false,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: 1280,
            height: 800,
        },
        bitmap: None,
        client_build: 0,
        client_name: "RemoteHub-spike".to_owned(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: MajorPlatformType::WINDOWS,
        enable_server_pointer: false,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        pointer_software_rendering: true,
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
    })
}

type UpgradedFramed =
    ironrdp_blocking::Framed<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>;

fn connect(
    config: connector::Config,
    server_name: String,
    port: u16,
) -> anyhow::Result<(ConnectionResult, UpgradedFramed)> {
    let server_addr = lookup_addr(&server_name, port).context("lookup addr")?;
    info!(%server_addr, "resolved server address");

    let tcp_stream = TcpStream::connect(server_addr).context("TCP connect")?;
    // Read timeout lets the active-stage loop break out once the desktop
    // has settled (no more graphics for 3s).
    tcp_stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .context("set_read_timeout")?;
    let client_addr = tcp_stream.local_addr().context("local addr")?;

    let mut framed = ironrdp_blocking::Framed::new(tcp_stream);
    let mut connector = connector::ClientConnector::new(config, client_addr);

    let should_upgrade =
        ironrdp_blocking::connect_begin(&mut framed, &mut connector).context("begin connection")?;

    debug!("TLS upgrade");
    let initial_stream = framed.into_inner_no_leftover();
    let (upgraded_stream, server_public_key) =
        tls_upgrade(initial_stream, server_name.clone()).context("TLS upgrade")?;

    let upgraded = ironrdp_blocking::mark_as_upgraded(should_upgrade, &mut connector);
    let mut upgraded_framed = ironrdp_blocking::Framed::new(upgraded_stream);
    let mut network_client = ReqwestNetworkClient;

    let connection_result = ironrdp_blocking::connect_finalize(
        upgraded,
        connector,
        &mut upgraded_framed,
        &mut network_client,
        server_name.into(),
        server_public_key,
        None,
    )
    .context("finalize connection (CredSSP)")?;

    Ok((connection_result, upgraded_framed))
}

fn active_stage(
    connection_result: ConnectionResult,
    mut framed: UpgradedFramed,
    image: &mut DecodedImage,
) -> anyhow::Result<()> {
    use std::time::Instant;

    let mut active_stage = ActiveStage::new(connection_result);
    let mut input_db = Database::new();
    let started = Instant::now();
    let mut received_any = false;

    // Phase machine: 0 = wait for desktop to settle; 1 = moved pointer,
    // wait a beat; 2 = clicked, pump a few seconds to capture the menu.
    let mut phase = 0u8;
    let mut phase_at = Instant::now();

    'outer: loop {
        let (action, payload) = match framed.read_pdu() {
            Ok((action, payload)) => (action, payload),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if phase == 0 && (received_any || started.elapsed() > Duration::from_secs(20)) {
                    info!("desktop settled → moving pointer to (400, 300)");
                    send_input(
                        &mut active_stage,
                        &mut input_db,
                        &mut framed,
                        image,
                        [Operation::MouseMove(MousePosition { x: 400, y: 300 })],
                    )?;
                    phase = 1;
                    phase_at = Instant::now();
                    continue;
                }
                if phase == 1 && phase_at.elapsed() > Duration::from_millis(400) {
                    info!("right-clicking at (400, 300)");
                    send_input(
                        &mut active_stage,
                        &mut input_db,
                        &mut framed,
                        image,
                        [Operation::MouseButtonPressed(IrMouseButton::Right)],
                    )?;
                    send_input(
                        &mut active_stage,
                        &mut input_db,
                        &mut framed,
                        image,
                        [Operation::MouseButtonReleased(IrMouseButton::Right)],
                    )?;
                    phase = 2;
                    phase_at = Instant::now();
                    continue;
                }
                if phase == 2 && phase_at.elapsed() > Duration::from_secs(3) {
                    break 'outer; // captured the post-click state
                }
                continue;
            }
            Err(e) => return Err(anyhow::Error::new(e).context("read frame")),
        };
        received_any = true;
        trace!(?action, frame_length = payload.len(), "frame received");

        let outputs = active_stage.process(image, action, &payload)?;
        for out in outputs {
            match out {
                ActiveStageOutput::ResponseFrame(frame) => {
                    framed.write_all(&frame).context("write response")?;
                }
                ActiveStageOutput::Terminate(_) => break 'outer,
                _ => {}
            }
        }
    }

    if !received_any {
        tracing::warn!("no frames received before timeout; PNG will be blank");
    }
    Ok(())
}

/// Encode UI operations into fast-path input PDUs via `ironrdp-input` and
/// push them through the active stage, writing any response frames.
///
/// NOTE: these symbol names (`Database::apply`, `Operation::*`,
/// `ActiveStage::process_fastpath_input`) are the spike's job to confirm
/// against ironrdp-input 0.5 / ironrdp-session 0.8 — if any mismatch, the
/// compiler error names the correct one and we adjust here before porting
/// the validated shape into the actor.
fn send_input(
    active_stage: &mut ActiveStage,
    input_db: &mut Database,
    framed: &mut UpgradedFramed,
    image: &mut DecodedImage,
    ops: impl IntoIterator<Item = Operation>,
) -> anyhow::Result<()> {
    let events = input_db.apply(ops);
    info!(fastpath_events = events.len(), "applied input ops");
    let outputs = active_stage
        .process_fastpath_input(image, &events)
        .context("process fastpath input")?;
    let mut frames = 0u32;
    for out in outputs {
        if let ActiveStageOutput::ResponseFrame(frame) = out {
            framed.write_all(&frame).context("write input frame")?;
            frames += 1;
        }
    }
    info!(response_frames = frames, "input frames written to server");
    Ok(())
}

fn lookup_addr(hostname: &str, port: u16) -> anyhow::Result<core::net::SocketAddr> {
    use std::net::ToSocketAddrs as _;
    (hostname, port)
        .to_socket_addrs()?
        .next()
        .context("socket address not found")
}

fn tls_upgrade(
    stream: TcpStream,
    server_name: String,
) -> anyhow::Result<(rustls::StreamOwned<rustls::ClientConnection, TcpStream>, Vec<u8>)> {
    let mut config = rustls::client::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(danger::NoCertificateVerification))
        .with_no_client_auth();
    config.key_log = std::sync::Arc::new(rustls::KeyLogFile::new());
    // CredSSP does not support TLS resumption.
    config.resumption = rustls::client::Resumption::disabled();
    let config = std::sync::Arc::new(config);

    let server_name = server_name.try_into()?;
    let client = rustls::ClientConnection::new(config, server_name)?;
    let mut tls_stream = rustls::StreamOwned::new(client, stream);
    // Flush to push the handshake forward so the peer cert is available.
    tls_stream.flush()?;

    let cert = tls_stream
        .conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .context("peer certificate missing")?;
    let server_public_key = extract_tls_server_public_key(cert)?;
    Ok((tls_stream, server_public_key))
}

fn extract_tls_server_public_key(cert: &[u8]) -> anyhow::Result<Vec<u8>> {
    use x509_cert::der::Decode as _;
    let cert = x509_cert::Certificate::from_der(cert)?;
    let key = cert
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .context("subject public key not byte-aligned")?
        .to_owned();
    Ok(key)
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
