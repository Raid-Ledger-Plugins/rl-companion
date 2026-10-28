//! System tray: icon + menu (status, Upload now, Pause/Resume, Open window, Quit).
//!
//! C-3 hooks: listen for [`UPLOAD_NOW_EVENT`] on the app handle, check
//! `Shared::is_paused()`, and call [`set_status`] with upload results.

use crate::state::Shared;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

/// Emitted when the user picks "Upload now".
pub const UPLOAD_NOW_EVENT: &str = "tray-upload-now";

const ID_STATUS: &str = "status";
const ID_UPLOAD: &str = "upload_now";
const ID_PAUSE: &str = "pause";
const ID_OPEN: &str = "open";
const ID_QUIT: &str = "quit";

/// Menu items whose text changes at runtime.
pub struct TrayItems {
    status: MenuItem<Wry>,
    pause: MenuItem<Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, ID_STATUS, "Not linked", false, None::<&str>)?;
    let upload = MenuItem::with_id(app, ID_UPLOAD, "Upload now", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, ID_PAUSE, "Pause uploads", true, None::<&str>)?;
    let open = MenuItem::with_id(app, ID_OPEN, "Open window", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit RL Companion", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&status, &sep, &upload, &pause, &open, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("RL Companion")
        .menu(&menu)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayItems { status, pause });
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        ID_UPLOAD => {
            let _ = app.emit(UPLOAD_NOW_EVENT, ());
        }
        ID_PAUSE => {
            let paused = app.state::<Shared>().toggle_paused();
            if let Some(items) = app.try_state::<TrayItems>() {
                let _ = items.pause.set_text(if paused { "Resume uploads" } else { "Pause uploads" });
            }
        }
        ID_OPEN => show_window(app),
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

/// Update the tray's status line. No-op before the tray exists (and in tests).
pub fn set_status(app: &AppHandle, text: &str) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.status.set_text(text);
    }
}
