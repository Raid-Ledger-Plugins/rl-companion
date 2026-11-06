//! System tray: icon + menu (status, Upload now, Pause/Resume, Open window,
//! Check for updates / Update to X, Quit).
//! Left-clicking the icon opens the window too.
//!
//! C-3 hooks: listen for [`UPLOAD_NOW_EVENT`] on the app handle, check
//! `Shared::is_paused()`, and call [`set_status`] with upload results.

use crate::state::Shared;
use crate::update_policy::{tray_label, Phase};
use crate::updater::Updates;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// Emitted when the user picks "Upload now".
pub const UPLOAD_NOW_EVENT: &str = "tray-upload-now";

const ID_STATUS: &str = "status";
const ID_UPLOAD: &str = "upload_now";
const ID_PAUSE: &str = "pause";
const ID_OPEN: &str = "open";
const ID_UPDATE: &str = "update";
const ID_QUIT: &str = "quit";

/// Menu items whose text changes at runtime.
pub struct TrayItems {
    status: MenuItem<Wry>,
    pause: MenuItem<Wry>,
    update: MenuItem<Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, ID_STATUS, "Not linked", false, None::<&str>)?;
    let upload = MenuItem::with_id(app, ID_UPLOAD, "Upload now", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, ID_PAUSE, "Pause uploads", true, None::<&str>)?;
    let open = MenuItem::with_id(app, ID_OPEN, "Open window", true, None::<&str>)?;
    let (update_text, _) = tray_label(&Phase::Idle);
    let update = MenuItem::with_id(app, ID_UPDATE, update_text, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit RL Companion", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&status, &sep, &upload, &pause, &open, &sep2, &update, &quit],
    )?;

    // Never build the tray without an image: an icon-less tray item is
    // invisible on Windows, which leaves no way back after closing the window.
    let icon = match app.default_window_icon() {
        Some(icon) => icon.clone(),
        None => tauri::include_image!("icons/32x32.png"),
    };
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("RL Companion")
        .menu(&menu)
        // Left click opens the window; the menu stays on right click.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if is_left_click(&event) {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayItems {
        status,
        pause,
        update,
    });
    Ok(())
}

/// A left-button release on the tray icon (a press would fire twice per click).
fn is_left_click(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        ID_UPLOAD => {
            let _ = app.emit(UPLOAD_NOW_EVENT, ());
        }
        ID_PAUSE => {
            let paused = app.state::<Shared>().toggle_paused();
            if let Some(items) = app.try_state::<TrayItems>() {
                let _ = items.pause.set_text(if paused {
                    "Resume uploads"
                } else {
                    "Pause uploads"
                });
            }
        }
        ID_OPEN => show_window(app),
        ID_UPDATE => {
            if let Some(u) = app.try_state::<Updates>() {
                u.click();
            }
        }
        ID_QUIT => app.exit(0),
        _ => {}
    }
}

/// Show and focus the main window (it hides to tray on close).
pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// The tray tooltip; spec §6.5 shows "Dev bridge on" while the listener is open.
pub fn tooltip(dev_bridge_on: bool) -> &'static str {
    if dev_bridge_on {
        "RL Companion — Dev bridge on"
    } else {
        "RL Companion"
    }
}

/// Tooltip + text badge beside the icon (macOS/Linux; Windows shows the
/// tooltip). No-op before the tray exists.
pub fn set_dev_bridge(app: &AppHandle, on: bool) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip(on)));
        let _ = tray.set_title(if on { Some("DEV") } else { None });
    }
}

/// Update the tray's status line. No-op before the tray exists (and in tests).
pub fn set_status(app: &AppHandle, text: &str) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.status.set_text(text);
    }
}

/// Point the update item at the updater's phase. No-op before the tray exists.
pub fn set_update(app: &AppHandle, phase: &Phase) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let (text, enabled) = tray_label(phase);
        let _ = items.update.set_text(text);
        let _ = items.update.set_enabled(enabled);
    }
}
