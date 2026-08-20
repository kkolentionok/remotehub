// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod kbd_hook;
mod logging;
mod paths;
mod local_pty;
mod rdp_session;
mod session;
mod forward_session;
mod sftp_session;
mod state;
mod sync_clock;
mod sync_engine;
mod sync_remote;
mod tray;

use std::sync::Arc;

use tauri::Manager;
use tracing::{error, info};

use rh_core::{CredentialStore, GroupStore, HostStore, KnownHostsStore, RdpCertStore, SettingsStore};
use rh_storage::{
    Db, OsKeychain, SqliteCredentialStore, SqliteForwardStore, SqliteGroupStore, SqliteHostStore,
    SqliteNoteStore, SqliteSnippetStore,
    SqliteKnownHostsStore, SqliteRdpCertStore, SqliteSettingsStore, SqliteSyncMetaStore,
};

use crate::state::AppState;

/// Force-release every keyboard modifier at the OS level (both L/R + the
/// generic VK, plus the Win keys). Bound to the global unstick hotkey so a
/// modifier stuck by another application can be cleared. Windows-only; a no-op
/// elsewhere.
#[cfg(windows)]
fn release_all_modifiers() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN,
        VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
    };

    // (virtual key, is-extended). Only the concrete L/R keys — NOT the generic
    // VK_CONTROL/VK_MENU/VK_SHIFT: injecting a KEYUP for a key that is already
    // up (or the ambiguous generic VK) confuses the OS keyboard state and can
    // latch Shift into a caps-lock-like mode. Right Ctrl/Alt and both Win keys
    // are extended-scancode keys.
    let keys: [(u16, bool); 8] = [
        (VK_LCONTROL, false),
        (VK_RCONTROL, true),
        (VK_LMENU, false),
        (VK_RMENU, true),
        (VK_LSHIFT, false),
        (VK_RSHIFT, false),
        (VK_LWIN, true),
        (VK_RWIN, true),
    ];

    // Release only keys the OS currently considers down — that catches a key
    // genuinely stuck by another app while leaving untouched ones alone.
    let mut inputs: Vec<INPUT> = keys
        .iter()
        .filter(|(vk, _)| unsafe { (GetAsyncKeyState(*vk as i32) as u16 & 0x8000) != 0 })
        .map(|&(vk, ext)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP | if ext { KEYEVENTF_EXTENDEDKEY } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
        .collect();

    let n = inputs.len();
    if n > 0 {
        // SAFETY: `inputs` is a valid, correctly-sized slice of INPUT.
        unsafe {
            SendInput(n as u32, inputs.as_mut_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }
    // Audible confirmation — plays even while another app (mstsc) is focused and
    // regardless of whether Pingie's window is hidden in the tray.
    beep();
    info!(released = n, "unstick hotkey");
}

/// Short confirmation tone. `kernel32!Beep` is declared directly here because
/// windows-sys 0.59 exposes neither `Beep` nor `MessageBeep`. It blocks for the
/// duration, so it runs on its own thread.
#[cfg(windows)]
fn beep() {
    #[link(name = "kernel32")]
    extern "system" {
        fn Beep(dwfreq: u32, dwduration: u32) -> i32;
    }
    std::thread::spawn(|| unsafe {
        Beep(1000, 120);
    });
}

#[cfg(not(windows))]
fn release_all_modifiers() {}

/// Re-bind (or disable) the global "unstick modifiers" hotkey at runtime.
/// Takes modifier flags + a W3C `KeyboardEvent.code` (e.g. "KeyK"); an empty
/// `code` disables the hotkey. Called by the settings UI; persisted client-side.
#[tauri::command]
fn set_unstick_hotkey(
    app: tauri::AppHandle,
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    code: Option<String>,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let Some(code) = code.filter(|c| !c.is_empty()) else {
        info!("unstick hotkey disabled");
        return Ok(());
    };
    let key: Code = code.parse().map_err(|_| format!("unknown key code: {code}"))?;
    let mut mods = Modifiers::empty();
    if ctrl {
        mods |= Modifiers::CONTROL;
    }
    if alt {
        mods |= Modifiers::ALT;
    }
    if shift {
        mods |= Modifiers::SHIFT;
    }
    if meta {
        mods |= Modifiers::SUPER;
    }
    let sc = Shortcut::new(if mods.is_empty() { None } else { Some(mods) }, key);
    gs.register(sc).map_err(|e| format!("register failed: {e}"))?;
    info!(?sc, "unstick hotkey re-registered");
    Ok(())
}

fn main() {
    if let Err(e) = logging::init() {
        eprintln!("warning: logging init failed: {e}");
    }
    // Capture panics to <app_data>/logs/panic.log even in the windowless
    // release build (panic = "abort" otherwise dies silently).
    logging::install_panic_hook();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        target = std::env::consts::OS,
        "Pingie starting"
    );

    tauri::Builder::default()
        // MUST be the first plugin: a second launch hands its args to this
        // callback instead of spawning a new process. The window may be hidden
        // in the tray (close-to-tray), so show + unminimize before focusing.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Global "unstick modifiers" hotkey. Fires system-wide (even when
        // Pingie is only in the tray) so a modifier stuck by ANOTHER app —
        // classically mstsc dropping Ctrl after Alt+Tab on a corporate RDP —
        // can be force-released while Pingie runs in the background.
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|_app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        release_all_modifiers();
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            // Closing the window must NOT quit the app: live SSH/RDP/SFTP
            // sessions (and any mounted drives / port-forwards) would drop.
            // Hide to the tray instead; real quit is via the tray menu.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // Build AppState synchronously on the Tokio runtime Tauri
            // already provides via tauri::async_runtime.
            let app_handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(async move {
                build_state(&app_handle).await
            });

            match state {
                Ok(s) => {
                    // Hand the background auto-sync actor its own clone (cheap —
                    // all-Arc) + the app handle for status events, then manage
                    // the original. The actor idles until sync is configured.
                    let sync_state = s.clone();
                    let sync_app = app.handle().clone();
                    let autostart_state = s.clone();
                    app.manage(s);
                    tauri::async_runtime::spawn(crate::sync_engine::run_loop(sync_app, sync_state));
                    // Bring up any forwards marked auto-start (best-effort).
                    tauri::async_runtime::spawn(async move {
                        crate::api::forwards::autostart_all(autostart_state).await;
                    });
                    info!("storage initialized; app ready");
                    if let Err(e) = tray::build(&app.handle().clone()) {
                        error!(error = %e, "failed to build system tray");
                    }
                    // Install the OS-level keyboard hook (Windows) for
                    // fullscreen RDP key capture. No-op elsewhere.
                    kbd_hook::init(app.handle().clone());

                    // Register the global "unstick modifiers" hotkey (Ctrl+Alt+K).
                    {
                        use tauri_plugin_global_shortcut::{
                            Code, GlobalShortcutExt, Modifiers, Shortcut,
                        };
                        let sc = Shortcut::new(
                            Some(Modifiers::CONTROL | Modifiers::ALT),
                            Code::KeyK,
                        );
                        match app.global_shortcut().register(sc) {
                            Ok(()) => info!("registered Ctrl+Alt+K (release stuck modifiers)"),
                            Err(e) => error!(error = %e, "failed to register unstick hotkey"),
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "FATAL: storage initialization failed; app cannot start");
                    // We still let the window open so the user sees
                    // *something* — a future stage will surface a
                    // proper "storage unavailable" dialog.
                    return Err(format!("storage init failed: {e}").into());
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Hosts
            api::hosts::host_list,
            api::hosts::host_get,
            api::hosts::known_host_get,
            api::hosts::known_hosts_list,
            api::hosts::known_host_forget,
            api::hosts::rdp_certs_list,
            api::hosts::rdp_cert_forget,
            api::hosts::host_create,
            api::hosts::host_update,
            api::hosts::host_delete,
            // Groups
            api::groups::group_list,
            api::groups::group_create,
            api::groups::group_rename,
            api::groups::group_move,
            api::groups::group_delete,
            // Credentials
            api::credentials::credential_list,
            api::credentials::credential_create,
            api::credentials::credential_update,
            api::credentials::credential_rotate_secret,
            api::credentials::credential_delete,
            api::credentials::credential_reveal,
            api::credentials::credential_link_host,
            api::credentials::credential_unlink_host,
            // Settings
            api::settings::settings_get_all,
            api::vault::vault_export,
            api::vault::vault_import,
            api::vault::vault_write_file,
            api::vault::vault_read_file,
            // Sync (slice 3b: server transport)
            api::sync::sync_get_config,
            api::sync::sync_set_endpoint,
            api::sync::sync_register,
            api::sync::sync_login,
            api::sync::sync_logout,
            api::sync::sync_oauth_yandex,
            api::sync::sync_set_master,
            api::sync::sync_status,
            api::ssh_id::ssh_id_get,
            api::ssh_id::ssh_id_set_handle,
            api::ssh_id::ssh_id_check,
            api::ssh_id::ssh_id_add_key,
            api::ssh_id::ssh_id_delete_key,
            api::ssh_id::ssh_id_update_label,
            api::ssh_id::ssh_id_available_keys,
            api::notes::note_list,
            api::notes::note_create,
            api::notes::note_update,
            api::notes::note_delete,
            api::notes::note_set_pinned,
            api::notes::note_set_fast_sync,
            api::sync::sync_refresh,
            api::snippets::snippet_list,
            api::snippets::snippet_create,
            api::snippets::snippet_update,
            api::snippets::snippet_delete,
            api::settings::settings_update,
            // Sessions (Stage 2: SSH)
            api::sessions::session_open,
            api::sessions::session_close,
            api::sessions::session_send_input,
            api::sessions::session_resize,
            api::sessions::session_accept_host_key,
            api::sessions::session_reject_host_key,
            api::sessions::session_list,
            api::forwards::forward_save,
            api::forwards::forward_start,
            api::forwards::forward_stop,
            api::forwards::forward_delete,
            api::forwards::forward_list,
            api::forwards::forward_set_auto_start,
            api::sessions::session_reattach,
            api::rdp_sessions::rdp_session_open,
            api::rdp_sessions::rdp_session_close,
            api::rdp_sessions::rdp_session_reattach,
            api::rdp_sessions::rdp_session_input,
            api::rdp_sessions::rdp_session_set_clipboard,
            api::rdp_sessions::rdp_session_set_clipboard_image,
            api::rdp_sessions::rdp_session_resize,
            api::rdp_sessions::rdp_session_kbd_capture,
            // Sessions (local shell PTY)
            api::local_sessions::local_session_open,
            api::local_sessions::local_session_close,
            api::local_sessions::local_session_input,
            api::local_sessions::local_session_resize,
            api::local_sessions::local_session_list,
            api::local_sessions::local_session_reattach,
            // Local filesystem (SFTP left pane)
            api::local_fs::fs_home,
            api::local_fs::fs_list,
            api::local_fs::fs_drives,
            api::local_fs::fs_rename,
            api::local_fs::fs_remove,
            api::local_fs::fs_mkdir,
            // SFTP (remote file browsing)
            api::sftp_sessions::sftp_open,
            api::sftp_sessions::sftp_list,
            api::sftp_sessions::sftp_close,
            api::sftp_sessions::sftp_download,
            api::sftp_sessions::sftp_upload,
            api::sftp_sessions::sftp_copy,
            api::sftp_sessions::sftp_rename,
            api::sftp_sessions::sftp_remove,
            api::sftp_sessions::sftp_transfer,
            api::sftp_sessions::sftp_transfer_cancel,
            api::sftp_sessions::sftp_mkdir,
            api::sftp_sessions::sftp_chmod,
            // Meta
            api::meta::app_version,
            set_unstick_hotkey,
            api::meta::ui_sessions_report,
            api::meta::app_quit,
            api::meta::open_external,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Construct the application state by opening the DB and wiring stores.
///
/// On schema mismatch in alpha mode, the DB is recreated; we log a
/// loud warning about data loss in that case.
async fn build_state(_app: &tauri::AppHandle) -> Result<AppState, String> {
    let data_dir = paths::app_data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create data dir {}: {e}", data_dir.display()))?;
    let db_path = data_dir.join("pingie.db");

    info!(db_path = %db_path.display(), "opening database");

    let (db, outcome) = Db::open(&db_path)
        .await
        .map_err(|e| format!("open database at {}: {e}", db_path.display()))?;

    use rh_storage::InitOutcome;
    match outcome {
        InitOutcome::Created => info!("created fresh database"),
        InitOutcome::AlreadyCurrent => info!("opened existing database at current schema"),
        InitOutcome::Recreated { old_version } => tracing::warn!(
            old_version,
            "database schema mismatch (alpha mode): existing data was wiped"
        ),
        InitOutcome::Migrated { old_version } => info!(
            old_version,
            "migrated database schema (data preserved)"
        ),
    }

    let hosts: Arc<dyn HostStore> = Arc::new(SqliteHostStore::new(db.clone()));
    let groups: Arc<dyn GroupStore> = Arc::new(SqliteGroupStore::new(db.clone()));
    let keychain = Arc::new(OsKeychain::new());
    let credentials: Arc<dyn CredentialStore> =
        Arc::new(SqliteCredentialStore::new(db.clone(), keychain));
    let settings: Arc<dyn SettingsStore> = Arc::new(SqliteSettingsStore::new(db.clone()));
    let known_hosts: Arc<dyn KnownHostsStore> = Arc::new(SqliteKnownHostsStore::new(db.clone()));
    let rdp_certs: Arc<dyn RdpCertStore> = Arc::new(SqliteRdpCertStore::new(db.clone()));
    let sync_meta: Arc<dyn rh_core::SyncMetaStore> = Arc::new(SqliteSyncMetaStore::new(db.clone()));
    let snippets: Arc<dyn rh_core::SnippetStore> = Arc::new(SqliteSnippetStore::new(db.clone()));
    let notes: Arc<dyn rh_core::NoteStore> = Arc::new(SqliteNoteStore::new(db.clone()));
    let forward_defs: Arc<dyn rh_core::ForwardStore> = Arc::new(SqliteForwardStore::new(db));
    let sync = Arc::new(crate::sync_clock::SyncClock::load_or_init(&data_dir));

    Ok(AppState::new(
        hosts, groups, snippets, notes, credentials, settings, known_hosts, rdp_certs, sync_meta,
        sync,
        forward_defs,
    ))
}
