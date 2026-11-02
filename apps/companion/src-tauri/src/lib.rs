//! Raid Ledger Companion — Tauri v2 shell (ROK-1961).
//!
//! Day-1 surface: tray, settings (server URL), device link + keychain token,
//! and the collector runtime (lane C-3): watch WoW's `LedgerLink.lua`, settle,
//! upload changed exports, honour the tray's Upload now / Pause.

pub mod bg3_collector;
pub mod collector;
pub mod collector_commands;
pub mod commands;
pub mod config;
pub mod link;
pub mod manifest;
pub mod poll;
pub mod registry;
pub mod runtime;
pub mod scope;
pub mod state;
pub mod token;
pub mod tray;
pub mod upload;
pub mod watch;
pub mod wow_collector;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use tauri::{Listener, Manager};

/// Build and run the app. Closing the window hides it to the tray; Quit is in
/// the tray menu.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let shared = state::Shared::new(
                config_dir.join(state::CONFIG_FILE),
                Box::new(token::KeyringStore),
            );
            app.manage(shared);
            tray::build(app.handle())?;
            let sent_path = app.path().app_data_dir()?.join(collector::SENT_STATE_FILE);
            let rt = Arc::new(runtime::Runtime::new(sent_path));
            app.manage(rt.clone());
            let kick = rt.clone();
            app.listen(tray::UPLOAD_NOW_EVENT, move |_| kick.kick());
            runtime::start(app.handle().clone(), rt);
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
            collector_commands::get_collector_status,
            collector_commands::set_wow_dir,
            collector_commands::pick_wow_dir,
            collector_commands::set_ledgerlink_enabled,
            collector_commands::upload_now,
        ])
        .run(tauri::generate_context!())
        .expect("error while running RL Companion");
}
