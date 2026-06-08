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
    Db, OsKeychain, SqliteCredentialStore, SqliteGroupStore, SqliteHostStore,
    SqliteKnownHostsStore, SqliteRdpCertStore, SqliteSettingsStore, SqliteSyncMetaStore,
};

use crate::state::AppState;

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
        "RemoteHub starting"
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
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
                    app.manage(s);
                    tauri::async_runtime::spawn(crate::sync_engine::run_loop(sync_app, sync_state));
                    info!("storage initialized; app ready");
                    if let Err(e) = tray::build(&app.handle().clone()) {
                        error!(error = %e, "failed to build system tray");
                    }
                    // Install the OS-level keyboard hook (Windows) for
                    // fullscreen RDP key capture. No-op elsewhere.
                    kbd_hook::init(app.handle().clone());
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
            api::settings::settings_update,
            // Sessions (Stage 2: SSH)
            api::sessions::session_open,
            api::sessions::session_close,
            api::sessions::session_send_input,
            api::sessions::session_resize,
            api::sessions::session_accept_host_key,
            api::sessions::session_reject_host_key,
            api::sessions::session_list,
            api::forwards::forward_open,
            api::forwards::forward_close,
            api::forwards::forward_list,
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
    let db_path = data_dir.join("remotehub.db");

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
    let sync_meta: Arc<dyn rh_core::SyncMetaStore> = Arc::new(SqliteSyncMetaStore::new(db));
    let sync = Arc::new(crate::sync_clock::SyncClock::load_or_init(&data_dir));

    Ok(AppState::new(
        hosts, groups, credentials, settings, known_hosts, rdp_certs, sync_meta, sync,
    ))
}
