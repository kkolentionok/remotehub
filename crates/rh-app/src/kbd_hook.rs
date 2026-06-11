//! OS-level keyboard capture for fullscreen RDP (Windows only).
//!
//! In fullscreen we want *every* keystroke — including system keys the OS
//! normally eats (the Windows key, Alt+Tab, Ctrl+Esc) — to go to the remote
//! desktop, exactly like mstsc's "Apply Windows key combinations: on the
//! remote computer". A web view can't do this: `preventDefault()` only blocks
//! browser-level defaults, not the OS Start menu, and the Keyboard Lock API
//! isn't reliable in WebView2 for the Win key.
//!
//! The proven way (and the whole reason we own the input path via IronRDP) is
//! a low-level keyboard hook, `WH_KEYBOARD_LL`. While capture is active (the
//! RDP canvas is fullscreen + our window is foreground) the hook swallows each
//! key locally and forwards its hardware scancode to the active RDP session.
//!
//! Safety: capture is only armed while fullscreen, and the hook additionally
//! verifies our window is the foreground window before swallowing — so it can
//! never hijack the keyboard of another app. Ctrl+Alt+Enter is always honored
//! to exit fullscreen (relayed to the UI), so the user can never get stuck.
//!
//! Non-Windows builds get no-op stubs; mac/Linux capture is future work.

use rh_core::SessionId;

/// Events sent from the (sync) hook callback to the async forwarder task.
#[allow(dead_code)]
enum HookEvent {
    /// A captured key, already a PS/2 Set 1 scancode + extended flag.
    Key {
        scancode: u8,
        extended: bool,
        pressed: bool,
    },
    /// Ctrl+Alt+Enter pressed — ask the UI to leave fullscreen.
    ExitFullscreen,
    /// Set (or clear) the session that captured keys are routed to.
    SetSession(Option<SessionId>),
}

#[cfg(windows)]
mod imp {
    use super::HookEvent;
    use rh_core::SessionId;
    use rh_rdp::RdpInputEvent;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::OnceLock;
    use tauri::{Emitter, Manager};
    use tokio::sync::mpsc::UnboundedSender;

    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU,
        VK_RSHIFT, VK_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
    };

    use crate::state::AppState;

    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
    static ALT_DOWN: AtomicBool = AtomicBool::new(false);
    /// Raw HWND of our main window (isize) — the foreground-window safety check.
    static APP_HWND: AtomicIsize = AtomicIsize::new(0);
    static FORWARD: OnceLock<UnboundedSender<HookEvent>> = OnceLock::new();

    unsafe extern "system" fn hook_proc(
        code: i32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        if code == HC_ACTION as i32 && ACTIVE.load(Ordering::Relaxed) {
            let fg = GetForegroundWindow() as isize;
            // Only intercept when *our* window is in front. If focus is
            // elsewhere, fall through so we never swallow another app's keys.
            if fg != 0 && fg == APP_HWND.load(Ordering::Relaxed) {
                let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
                let vk = kb.vkCode;
                let msg = wparam as u32;
                let pressed = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;

                // Track Ctrl/Alt so we can recognize the exit shortcut.
                if vk == VK_CONTROL as u32 || vk == VK_LCONTROL as u32 || vk == VK_RCONTROL as u32 {
                    CTRL_DOWN.store(pressed, Ordering::Relaxed);
                } else if vk == VK_MENU as u32 || vk == VK_LMENU as u32 || vk == VK_RMENU as u32 {
                    ALT_DOWN.store(pressed, Ordering::Relaxed);
                }

                // Reserved client shortcut: Ctrl+Alt+Enter exits fullscreen.
                // Handled here (not forwarded) so it always works.
                if pressed
                    && vk == VK_RETURN as u32
                    && CTRL_DOWN.load(Ordering::Relaxed)
                    && ALT_DOWN.load(Ordering::Relaxed)
                {
                    if let Some(tx) = FORWARD.get() {
                        let _ = tx.send(HookEvent::ExitFullscreen);
                    }
                    return 1; // swallow
                }

                let extended = (kb.flags & LLKHF_EXTENDED) != 0;
                // Pure modifier keys (Shift/Ctrl/Alt) are forwarded to the
                // remote AND passed through to the local OS (not swallowed).
                // EMPIRICALLY REQUIRED: fully swallowing them (mstsc-style) broke
                // RU comma (Shift+/) — confirmed twice. The exact dependency
                // isn't understood (scancodes *should* be layout-independent),
                // so DO NOT remove this without a live test of: switch layout
                // Alt+Shift in fullscreen → type RU comma. Known downside: the
                // passed-through modifiers leak into our own webview — a lone
                // Ctrl can fire the Ctrl+K search and Alt+Shift can steal input
                // focus. Fixing those WITHOUT regressing comma needs the fast
                // build cycle (try: swallow Ctrl only, keep Shift+Alt; or disable
                // the app's own Ctrl+K handler while RDP capture is active).
                // Printable keys are still swallowed. Win stays swallowed.
                let modifier_passthrough = vk == VK_SHIFT as u32
                    || vk == VK_LSHIFT as u32
                    || vk == VK_RSHIFT as u32
                    || vk == VK_CONTROL as u32
                    || vk == VK_LCONTROL as u32
                    || vk == VK_RCONTROL as u32
                    || vk == VK_MENU as u32
                    || vk == VK_LMENU as u32
                    || vk == VK_RMENU as u32;
                if let Some(tx) = FORWARD.get() {
                    let _ = tx.send(HookEvent::Key {
                        scancode: kb.scanCode as u8,
                        extended,
                        pressed,
                    });
                }
                if modifier_passthrough {
                    return CallNextHookEx(ptr::null_mut(), code, wparam, lparam);
                }
                return 1; // swallow locally; delivered to the remote instead
            }
        }
        CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
    }

    pub fn init(app: tauri::AppHandle) {
        // Record our main window's HWND for the foreground check.
        if let Some(w) = app.get_webview_window("main") {
            if let Ok(h) = w.hwnd() {
                APP_HWND.store(h.0 as isize, Ordering::Relaxed);
            }
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<HookEvent>();
        if FORWARD.set(tx).is_err() {
            return; // already initialized
        }

        // Forwarder: pushes captured keys to the active RDP session and relays
        // the fullscreen-exit shortcut to the UI. Runs on the Tokio runtime.
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut current: Option<SessionId> = None;
            while let Some(ev) = rx.recv().await {
                match ev {
                    HookEvent::SetSession(s) => current = s,
                    HookEvent::ExitFullscreen => {
                        let _ = app2.emit("rdp:exit-fullscreen", ());
                    }
                    HookEvent::Key {
                        scancode,
                        extended,
                        pressed,
                    } => {
                        if let Some(sid) = current.clone() {
                            let state = app2.state::<AppState>();
                            state
                                .rdp_sessions
                                .send_input(
                                    &sid,
                                    RdpInputEvent::RawScancode {
                                        scancode,
                                        extended,
                                        pressed,
                                    },
                                )
                                .await;
                        }
                    }
                }
            }
        });

        // Dedicated thread: install the hook and pump its message loop (a
        // low-level hook requires a message loop on the installing thread).
        std::thread::spawn(|| unsafe {
            let hmod = GetModuleHandleW(ptr::null());
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hmod, 0);
            if hook.is_null() {
                tracing::error!("kbd_hook: SetWindowsHookExW failed; fullscreen key capture off");
                return;
            }
            tracing::info!("kbd_hook: low-level keyboard hook installed");
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {}
            let _ = UnhookWindowsHookEx(hook);
        });
    }

    pub fn set_capture(on: bool, session: Option<SessionId>) {
        ACTIVE.store(on, Ordering::Relaxed);
        if let Some(tx) = FORWARD.get() {
            let _ = tx.send(HookEvent::SetSession(if on { session } else { None }));
        }
        if !on {
            // Forget tracked modifiers so a stale Ctrl/Alt can't linger.
            CTRL_DOWN.store(false, Ordering::Relaxed);
            ALT_DOWN.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(windows)]
pub use imp::{init, set_capture};

#[cfg(not(windows))]
pub fn init(_app: tauri::AppHandle) {}

#[cfg(not(windows))]
pub fn set_capture(_on: bool, _session: Option<SessionId>) {}
