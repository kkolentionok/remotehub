//! System-tray icon + right-click context menu.
//!
//! The menu is built from a snapshot of the host/group stores: a **Recent**
//! submenu (hosts with a `last_connected_at`, newest first), a **Groups**
//! submenu (one nested submenu per group), plus Open / Quit. Selecting a host
//! emits `tray:connect` with its id; the frontend then opens it through the
//! normal session flow (so all the connect logic stays in one place).
//!
//! The menu is rebuilt whenever hosts or groups change (we listen to the same
//! `hosts:changed` / `groups:changed` events the UI gets), so the tray stays
//! in sync without the UI being open.
//!
//! NOTE (Tauri version-sensitive): the `menu` + `tray` builder APIs are the
//! fragile part here. If this fails to compile, the likely culprits are
//! `show_menu_on_left_click` (was `menu_on_left_click` pre-2.1) and the
//! `SubmenuBuilder::new(manager, text)` / `.item()` / `.build()` shapes.

use tauri::{
    menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Listener, Manager, Wry,
};

use std::sync::atomic::Ordering;

use rh_core::{Host, HostFilter};

use crate::api::events::names;
use crate::state::AppState;

/// How many recent hosts to list in the tray.
const RECENT_LIMIT: usize = 8;

/// Build the tray icon and register it. Call once from `setup`, after the
/// `AppState` has been managed.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(tooltip_for(0))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == "quit" {
                // If the UI has live session tabs open, don't quit outright —
                // surface the window and let the frontend show a confirm. The
                // real exit comes back through the `app_quit` command. When
                // nothing is live, quit immediately.
                let live = app
                    .try_state::<AppState>()
                    .map(|s| s.session_count.load(Ordering::Relaxed))
                    .unwrap_or(0);
                if live > 0 {
                    show_main(app);
                    let _ = app.emit("app:confirm-quit", live as u32);
                } else {
                    app.exit(0);
                }
            } else if id == "show" {
                show_main(app);
            } else if let Some(host_id) = id.strip_prefix("connect:") {
                let _ = app.emit("tray:connect", host_id.to_string());
                show_main(app);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    // Reuse the configured app icon for the tray.
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;

    // Keep the menu fresh as the catalog changes.
    let app_for_hosts = app.clone();
    app.listen(names::HOSTS_CHANGED, move |_| rebuild(&app_for_hosts));
    let app_for_groups = app.clone();
    app.listen(names::GROUPS_CHANGED, move |_| rebuild(&app_for_groups));

    Ok(())
}

/// Rebuild and swap in a fresh menu (best-effort; logs nothing on failure to
/// avoid noise if the tray was removed).
///
/// **Must run on the main (UI) thread.** The `hosts:changed`/`groups:changed`
/// listeners fire on the Tokio worker that emitted the event; building/setting
/// the native menu off the UI thread corrupts thread-affine Win32 menu state
/// and hard-crashes the process (`0xC0000409`, no Rust panic — it bypasses the
/// panic hook). Deferring to the main thread also makes `build_menu`'s
/// `block_on` safe, since the main thread is not a runtime worker (same context
/// as the initial `build()` in `setup`, which has always worked).
fn rebuild(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Ok(menu) = build_menu(&app) {
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_menu(Some(menu));
            }
        }
    });
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Tray tooltip text for a given live-session count.
fn tooltip_for(n: usize) -> String {
    match n {
        0 => "RemoteHub".to_string(),
        1 => "RemoteHub · 1 active session".to_string(),
        _ => format!("RemoteHub · {n} active sessions"),
    }
}

/// Refresh the tray tooltip with the current live-session count. Best-effort;
/// silently no-ops if the tray was removed. Called from `ui_sessions_report`
/// (a Tokio worker), so — like `rebuild` — the native mutation is deferred to
/// the main (UI) thread.
pub fn update_tooltip(app: &AppHandle, n: usize) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(tooltip_for(n)));
        }
    });
}

fn label(h: &Host) -> String {
    h.display_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| h.hostname.clone())
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<AppState>();
    let (hosts, groups) = tauri::async_runtime::block_on(async {
        let hosts = state
            .hosts
            .list(HostFilter::default())
            .await
            .unwrap_or_default();
        let groups = state.groups.list().await.unwrap_or_default();
        (hosts, groups)
    });

    let mut b = MenuBuilder::new(app);
    b = b.item(&MenuItemBuilder::with_id("show", "Open RemoteHub").build(app)?);

    // Favorites — user-pinned hosts, by name.
    let mut favs: Vec<&Host> = hosts.iter().filter(|h| h.favorite).collect();
    favs.sort_by(|a, c| label(a).to_lowercase().cmp(&label(c).to_lowercase()));
    if !favs.is_empty() {
        let mut sub = SubmenuBuilder::new(app, "Favorites");
        for h in favs {
            sub = sub.item(&MenuItemBuilder::with_id(format!("connect:{}", h.id), label(h)).build(app)?);
        }
        b = b.item(&sub.build()?);
    }

    // Recent — hosts that have been connected to, newest first.
    let mut recent: Vec<&Host> = hosts
        .iter()
        .filter(|h| h.last_connected_at.is_some())
        .collect();
    recent.sort_by(|a, c| c.last_connected_at.cmp(&a.last_connected_at));
    if !recent.is_empty() {
        let mut sub = SubmenuBuilder::new(app, "Recent");
        for h in recent.into_iter().take(RECENT_LIMIT) {
            sub = sub.item(&MenuItemBuilder::with_id(format!("connect:{}", h.id), label(h)).build(app)?);
        }
        b = b.item(&sub.build()?);
    }

    // Groups — one nested submenu per group (skipping empty groups).
    if !groups.is_empty() {
        let mut groups_sub = SubmenuBuilder::new(app, "Groups");
        let mut any_group = false;
        for g in &groups {
            let mut gs = SubmenuBuilder::new(app, g.name.clone());
            let mut any = false;
            for h in hosts.iter().filter(|h| h.group_id.as_ref() == Some(&g.id)) {
                gs = gs.item(&MenuItemBuilder::with_id(format!("connect:{}", h.id), label(h)).build(app)?);
                any = true;
            }
            if any {
                groups_sub = groups_sub.item(&gs.build()?);
                any_group = true;
            }
        }
        if any_group {
            b = b.item(&groups_sub.build()?);
        }
    }

    b = b.separator();
    b = b.item(&MenuItemBuilder::with_id("quit", "Quit").build(app)?);
    b.build()
}
