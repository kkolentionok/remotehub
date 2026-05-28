// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod logging;
mod paths;
mod state;

use std::sync::Arc;

use tauri::Manager;
use tracing::{error, info};

use rh_core::{CredentialStore, GroupStore, HostStore, SettingsStore};
use rh_storage::{
    Db, OsKeychain, SqliteCredentialStore, SqliteGroupStore, SqliteHostStore, SqliteSettingsStore,
};

use crate::state::AppState;

fn main() {
    if let Err(e) = logging::init() {
        eprintln!("warning: logging init failed: {e}");
    }

    info!(
        version = env!("CARGO_PKG_VERSION"),
        target = std::env::consts::OS,
        "RemoteHub starting"
    );

    tauri::Builder::default()
        .setup(|app| {
            // Build AppState synchronously on the Tokio runtime Tauri
            // already provides via tauri::async_runtime.
            let app_handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(async move {
                build_state(&app_handle).await
            });

            match state {
                Ok(s) => {
                    app.manage(s);
                    info!("storage initialized; app ready");
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
            api::settings::settings_update,
            // Sessions (stubs in Stage 1.4)
            api::sessions::session_open,
            api::sessions::session_close,
            api::sessions::session_list,
            // Meta
            api::meta::app_version,
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
    }

    let hosts: Arc<dyn HostStore> = Arc::new(SqliteHostStore::new(db.clone()));
    let groups: Arc<dyn GroupStore> = Arc::new(SqliteGroupStore::new(db.clone()));
    let keychain = Arc::new(OsKeychain::new());
    let credentials: Arc<dyn CredentialStore> =
        Arc::new(SqliteCredentialStore::new(db.clone(), keychain));
    let settings: Arc<dyn SettingsStore> = Arc::new(SqliteSettingsStore::new(db));

    Ok(AppState::new(hosts, groups, credentials, settings))
}
