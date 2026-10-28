//! Raid Ledger Companion — Tauri v2 shell (ROK-1961).
//!
//! Day-1 surface: tray, settings (server URL), device link + keychain token.
//! The collector runtime (watch, settle, upload) lands in lane C-3 and plugs
//! into [`state::Shared`] and the tray's `upload-now` / pause hooks.

pub mod commands;
pub mod config;
pub mod link;
pub mod poll;
pub mod state;
pub mod token;
pub mod tray;

#[cfg(test)]
mod tests;

use tauri::Manager;

/// Build and run the app. Closing the window hides it to the tray; Quit is in
/// the tray menu.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let shared = state::Shared::new(config_dir.join(state::CONFIG_FILE), Box::new(token::KeyringStore));
            app.manage(shared);
            tray::build(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::check_link,
            commands::set_server_url,
            commands::set_device_name,
            commands::start_link,
            commands::cancel_link,
            commands::open_verification,
            commands::unlink,
        ])
        .run(tauri::generate_context!())
        .expect("error while running RL Companion");
}
