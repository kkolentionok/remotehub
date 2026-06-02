//! RDP session actor for RemoteHub.
//!
//! Stage 4. This module currently defines the **contract** — the input
//! events the UI sends, the session events the actor emits, and the
//! spawn/credential/option types. The IronRDP connect + graphics decode
//! loop lands in a follow-up slice, after the connectivity/transport
//! spike the spec mandates (see `docs/specs/rdp-session.md`, Open Qs).
//!
//! Layering mirrors `rh-ssh`: this crate depends only on `rh-core` and
//! transport-agnostic primitives. The `rh-app` layer bridges the event
//! channel to a Tauri `ipc::Channel`.

use serde::{Deserialize, Serialize};

use rh_core::{Host, RevealedSecret, SessionId};

mod actor;
pub use actor::spawn_session;

/// Commands the UI/host layer sends to a running RDP session actor.
pub enum RdpCommand {
    Input(RdpInputEvent),
    Resize { width: u16, height: u16 },
    /// Push the local OS clipboard text to the session, so it can be pasted
    /// into the remote desktop (CLIPRDR client→server).
    SetClipboard(String),
    /// Push a local OS clipboard image (raw RGBA, top-down) to the session for
    /// paste into the remote desktop (offered to the server as CF_DIB).
    SetClipboardImage {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// User-requested graceful close.
    Shutdown,
}

// =====================================================================
// Input — UI → actor
// =====================================================================

/// A pointer button. Maps to `ironrdp-input` mouse buttons in the actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Input events sent by the UI. `kind`-tagged to match the TS union.
///
/// Keyboard: the UI sends the **browser** `KeyboardEvent.code` string
/// (e.g. "KeyA", "ArrowUp"); the actor maps it to a PS/2 scancode (single
/// source of truth, keeps the UI dumb).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RdpInputEvent {
    MouseMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
        x: u16,
        y: u16,
    },
    MouseWheel {
        /// Vertical wheel delta (positive = up). Horizontal omitted for MVP.
        delta: i16,
        x: u16,
        y: u16,
    },
    Key {
        /// Browser `KeyboardEvent.code`, e.g. "KeyA", "Enter", "ArrowUp".
        code: String,
        pressed: bool,
        /// Auto-repeat (held key). Servers generally want these too.
        #[serde(default)]
        repeat: bool,
    },

    /// A key already resolved to a PS/2 **Set 1** scancode by the OS-level
    /// keyboard hook (Windows `WH_KEYBOARD_LL`). Used in fullscreen so system
    /// keys (Win, Alt+Tab, …) reach the remote instead of the local OS.
    /// Bypasses `code_to_scancode` — the hook hands us the hardware scancode
    /// and the extended flag directly.
    RawScancode {
        scancode: u8,
        extended: bool,
        pressed: bool,
    },

    /// Sent when the RDP canvas gains focus: the full state of physical
    /// modifier keys at that moment. The actor diffs against its internal
    /// `ModifierState` and emits the KeyDown/KeyUp needed to resync — the
    /// fix for the classic "stuck modifier" bug. Lock keys additionally
    /// drive a `TS_SYNC_EVENT`.
    SyncModifiers {
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
        caps_lock: bool,
        num_lock: bool,
        scroll_lock: bool,
    },

    /// Sent on focus loss. The actor releases every modifier the server
    /// believes is held — no diff, blanket KeyUp.
    ReleaseAllModifiers,
}

/// Actor-side tracking of which modifiers the server currently believes
/// are held, so `SyncModifiers` can compute a minimal diff and
/// `ReleaseAllModifiers` knows what to release. (Used by the actor in the
/// follow-up slice; defined here as part of the contract.)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModifierState {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub scroll_lock: bool,
}

// =====================================================================
// Output — actor → UI
// =====================================================================

/// Pixel layout of a decoded frame. We negotiate whichever the browser's
/// `ImageData` consumes without a per-pixel swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Bgra8,
    Rgba8,
}

/// A decoded rectangle of the framebuffer, row-major, no padding.
#[derive(Debug, Clone, Serialize)]
pub struct FrameRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// Transport encoding for a frame region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// Lossless — crisp text/UI. Used for small regions.
    Png,
    /// Lossy but compact — used for large/photo-like regions.
    Jpeg,
}

/// One changed rectangle within a `FrameBatch`, compressed + base64-encoded.
/// The UI builds a `data:image/{format}` URL and `drawImage`s it at (x, y).
#[derive(Debug, Clone, Serialize)]
pub struct FrameTile {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub format: ImageFormat,
    pub base64: String,
}

/// Lifecycle states for an RDP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RdpState {
    /// Resolving the host name (DNS).
    Resolving,
    /// TCP connect + RDP negotiation + TLS upgrade — reaching the host.
    Connecting,
    /// CredSSP/NTLM credential exchange.
    Authenticating,
    Ready,
    Closed,
}

/// Why a session ended. Mirrors the SSH `CloseReason` shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RdpCloseReason {
    UserRequested,
    ServerDisconnected { code: Option<u32> },
    AuthFailed,
    CertRejected,
    Error,
}

/// Events emitted by the actor to the UI. `kind`-tagged to match TS.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RdpSessionEvent {
    StateChanged {
        state: RdpState,
    },
    /// A decoded framebuffer region. The UI builds `ImageData` from
    /// `data` (length == width*height*4) and `putImageData` at (x, y).
    Frame {
        region: FrameRegion,
        format: PixelFormat,
        data: Vec<u8>,
    },
    /// All changed tiles of a single frame, sent together so the UI can draw
    /// them in one synchronous pass — the frame appears whole instead of
    /// band-by-band (which tore the image during fast motion). Region-diff
    /// keeps the tile set small; each tile is PNG (crisp text) or JPEG
    /// (compact, large areas), base64-encoded.
    FrameBatch {
        tiles: Vec<FrameTile>,
    },
    /// The server's actual desktop size (after negotiation or a live resize).
    /// The UI sizes its canvas to match so region coordinates line up.
    Resized {
        width: u16,
        height: u16,
    },
    /// Pointer position update from the server (e.g. warp). Cursor-shape
    /// updates are post-MVP.
    PointerPosition {
        x: u16,
        y: u16,
    },
    /// Server cursor shape changed. `rgba_base64` is non-premultiplied,
    /// top-down RGBA (width*height*4 bytes) base64-encoded; the UI turns it
    /// into a PNG `data:` URL and sets it as the canvas CSS cursor, offset by
    /// the hotspot — so the remote cursor follows the local mouse instantly.
    PointerBitmap {
        width: u16,
        height: u16,
        hotspot_x: u16,
        hotspot_y: u16,
        rgba_base64: String,
    },
    /// Server hid the cursor (e.g. full-screen games, text fields).
    PointerHidden,
    /// Server reverted to the default arrow cursor.
    PointerDefault,
    /// Unknown/untrusted server certificate — UI prompts the user.
    CertPrompt {
        fingerprint_sha256: String,
        subject: String,
    },
    /// Plain-text clipboard from the server (MVP: text only).
    Clipboard {
        mime: String,
        data: String,
    },
    Error {
        message: String,
    },
    Closed {
        reason: RdpCloseReason,
    },
}

// =====================================================================
// Spawn params
// =====================================================================

/// Color depth negotiated with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorDepth {
    Depth16,
    Depth24,
    Depth32,
}

#[derive(Debug, Clone)]
pub struct RdpOpenOptions {
    pub width: u16,
    pub height: u16,
    pub color_depth: ColorDepth,
    /// Windows keyboard-layout id (hint to the server in Client Info PDU).
    pub keyboard_layout: u32,
    pub enable_clipboard: bool,
}

/// Credential revealed for an RDP login. (SmartCard/cert — post-MVP.)
pub enum RevealedRdpCredential {
    Password {
        username: String,
        domain: Option<String>,
        password: RevealedSecret,
    },
}

/// Everything the actor needs to open an RDP session. Mirrors the SSH
/// pattern: events flow out over an mpsc channel that `rh-app` bridges to
/// a Tauri `ipc::Channel`.
pub struct RdpSpawnParams {
    pub id: SessionId,
    pub host: Host,
    pub credential: RevealedRdpCredential,
    pub options: RdpOpenOptions,
}

// =====================================================================
// Errors
// =====================================================================

#[derive(Debug, thiserror::Error)]
pub enum RdpError {
    #[error("network error: {0}")]
    Network(#[from] std::io::Error),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("connector error: {0}")]
    Connector(String),

    #[error("authentication failed")]
    AuthFailed,

    #[error("server certificate is untrusted")]
    CertUntrusted,

    #[error("server certificate rejected by user")]
    CertRejected,

    #[error("active stage error: {0}")]
    ActiveStage(String),

    #[error("PDU decode error: {0}")]
    Decode(String),

    #[error("graphics decode error: {0}")]
    Graphics(String),
}

impl RdpError {
    /// Map to the close reason surfaced to the UI.
    #[must_use]
    pub fn into_close_reason(self) -> RdpCloseReason {
        match self {
            RdpError::AuthFailed => RdpCloseReason::AuthFailed,
            RdpError::CertUntrusted | RdpError::CertRejected => RdpCloseReason::CertRejected,
            _ => RdpCloseReason::Error,
        }
    }
}
