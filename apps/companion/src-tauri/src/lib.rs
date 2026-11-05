//! Raid Ledger Companion — Tauri v2 shell (ROK-1961).
//!
//! Day-1 surface: tray, settings (server URL), device link + keychain token,
//! and the collector runtime (lane C-3): watch WoW's `LedgerLink.lua`, settle,
//! upload changed exports, honour the tray's Upload now / Pause.

pub mod autostart;
pub mod bg3_campaigns;
pub mod bg3_collector;
pub mod bg3_screenshot;
pub mod blob_send;
pub mod collector;
pub mod collector_commands;
pub mod collector_view;
pub mod commands;
pub mod config;
pub mod covers;
pub mod dev_bridge;
pub mod gate;
pub mod link;
pub mod manifest;
pub mod poll;
pub mod preview;
pub mod registry;
pub mod runtime;
pub mod scope;
pub mod screenshot_commands;
pub mod screenshot_encode;
pub mod screenshot_preview;
pub mod server_config;
pub mod state;
pub mod tiles;
pub mod token;
pub mod tray;
pub mod tray_promote;
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
        // Must be first (plugin docs): a second launch hands its argv to this
        // process and exits, so two runtimes never watch the same files.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if autostart::should_show_window(&argv) {
                tray::show_window(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![autostart::AUTOSTART_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let shared = state::Shared::new(
                config_dir.join(state::CONFIG_FILE),
                Box::new(token::KeyringStore),
            );
            let start_cfg = shared.config();
            app.manage(shared);
            // Dev bridge (ROK-2055): reconcile opens nothing while it is off.
            let bridge = Arc::new(dev_bridge::DevBridge::new(
                config_dir.join(dev_bridge::TLS_FILE),
                app.path().app_data_dir().ok().as_deref(),
            ));
            app.manage(bridge.clone());
            // `/dev/v1/bg3/*` (ROK-2078): the same Story roots the bg3-save collector watches.
            // `set_collector_root` rebinds it when the folder changes.
            bridge.set_bg3_from_config(&start_cfg, app.path().app_data_dir().ok());
            tray::build(app.handle())?;
            let (tray_app, start_bridge) = (app.handle().clone(), bridge.clone());
            tauri::async_runtime::spawn(async move {
                let s = start_bridge.reconcile(&start_cfg).await;
                tray::set_dev_bridge(&tray_app, s.listening.is_some());
            });
            tray_promote::start(app.handle());
            let sent_path = app.path().app_data_dir()?.join(collector::SENT_STATE_FILE);
            let rt = Arc::new(runtime::Runtime::new(sent_path));
            app.manage(rt.clone());
            let (src_app, src_rt) = (app.handle().clone(), rt.clone());
            bridge.set_collectors_source(Arc::new(move || {
                let paused = src_app.state::<state::Shared>().is_paused();
                let status = runtime::status(&src_app, &src_rt);
                dev_bridge::collectors::collectors_body(&status, paused)
            }));
            let kick = rt.clone();
            app.listen(tray::UPLOAD_NOW_EVENT, move |_| kick.kick());
            runtime::start(app.handle().clone(), rt.clone());
            server_config::start(app.handle().clone(), rt);
            autostart::init_default(app.handle());
            // The window starts hidden (tauri.conf.json); a login-item launch
            // leaves it in the tray, any other launch shows it.
            if autostart::should_show_window(autostart::process_args()) {
                tray::show_window(app.handle());
            }
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
            collector_commands::set_collector_enabled,
            collector_commands::set_collector_root,
            collector_commands::pick_collector_root,
            collector_commands::confirm_first_upload,
            collector_commands::skip_first_upload,
            collector_commands::refresh_server_config,
            collector_commands::set_bg3_campaign,
            screenshot_commands::get_bg3_screenshot_hubs,
            screenshot_commands::set_bg3_screenshot_hub,
            dev_bridge::commands::get_dev_bridge_status,
            dev_bridge::commands::set_dev_bridge_enabled,
            dev_bridge::commands::set_dev_bridge_probes_enabled,
            dev_bridge::commands::set_dev_bridge_bind,
            dev_bridge::commands::set_dev_bridge_allowlist,
            dev_bridge::commands::rotate_dev_bridge_token,
        ])
        .build(tauri::generate_context!())
        .expect("error while building RL Companion")
        .run(|app, event| {
            // macOS: clicking the Dock icon while the window is hidden sends
            // Reopen, not a second launch, so single-instance never sees it.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = event
            {
                tray::show_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
