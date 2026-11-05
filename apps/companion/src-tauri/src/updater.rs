//! In-app updates: check `latest.json` at startup and every few hours,
//! download a newer signed build straight away, and install it when the app
//! is idle or the user clicks "Update now". The rules live in
//! [`crate::update_policy`]; this is the Tauri side.
//!
//! One task owns every check and install, so they never overlap; the window
//! commands and the tray item only send it [`Request`]s.

use crate::dev_bridge::DevBridge;
use crate::runtime::Runtime;
use crate::update_policy::{
    click_action, is_idle, next_check_in, percent, Action, Channel, IdleInputs, Phase,
    UpdateStatus, FIRST_CHECK_DELAY, IDLE_POLL, SILENT_RESTART_MARKER,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

/// Emitted with an [`UpdateStatus`] whenever it changes.
pub const UPDATE_STATUS_EVENT: &str = "update-status";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Check,
    InstallNow,
}

/// Managed state: the current status and the way to reach the update task.
pub struct Updates {
    status: Mutex<UpdateStatus>,
    tx: UnboundedSender<Request>,
}

impl Updates {
    pub fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn request(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    /// The window button / tray item: install if ready, else check.
    pub fn click(&self) {
        match click_action(&self.status().phase) {
            Action::Check => self.request(Request::Check),
            Action::InstallNow => self.request(Request::InstallNow),
            Action::Nothing => {}
        }
    }
}

/// The running build's version and channel.
fn current(app: &AppHandle) -> (String, Channel) {
    let v = app.package_info().version.clone();
    (v.to_string(), Channel::for_version(&v))
}

/// Register the state and spawn the update task. Call once from `setup`.
pub fn start(app: &AppHandle) {
    let (current_version, channel) = current(app);
    let (tx, rx) = unbounded_channel();
    app.manage(Updates {
        status: Mutex::new(UpdateStatus {
            current_version,
            channel,
            phase: Phase::Idle,
            last_checked: None,
        }),
        tx,
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move { run(app, rx).await });
}

/// Set the phase, refresh the tray item and tell the window.
fn set_phase(app: &AppHandle, phase: Phase, checked: bool) {
    let Some(u) = app.try_state::<Updates>() else {
        return;
    };
    let status = {
        let mut s = u.status.lock().unwrap_or_else(|p| p.into_inner());
        s.phase = phase;
        if checked {
            s.last_checked = Some(crate::runtime::now());
        }
        s.clone()
    };
    crate::tray::set_update(app, &status.phase);
    let _ = app.emit(UPDATE_STATUS_EVENT, status);
}

/// A downloaded, verified update waiting to install.
struct Pending {
    update: Update,
    bytes: Vec<u8>,
}

async fn run(app: AppHandle, mut rx: UnboundedReceiver<Request>) {
    let mut next_check = Instant::now() + FIRST_CHECK_DELAY;
    let mut pending: Option<Pending> = None;
    let mut idle_tick = tokio::time::interval(IDLE_POLL);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next_check) => {
                next_check = Instant::now() + check(&app, &mut pending).await;
            }
            Some(r) = rx.recv() => match r {
                Request::Check => {
                    next_check = Instant::now() + check(&app, &mut pending).await;
                }
                Request::InstallNow => {
                    if let Some(p) = pending.take() {
                        install(&app, p, false);
                    }
                }
            },
            _ = idle_tick.tick(), if pending.is_some() => {
                if is_idle(&idle_inputs(&app)) {
                    if let Some(p) = pending.take() {
                        install(&app, p, true);
                    }
                }
            }
        }
    }
}

fn idle_inputs(app: &AppHandle) -> IdleInputs {
    IdleInputs {
        scanning: app
            .try_state::<Arc<Runtime>>()
            .is_some_and(|rt| rt.is_scanning()),
        window_visible: app
            .get_webview_window("main")
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false),
        since_bridge_request: app
            .try_state::<Arc<DevBridge>>()
            .and_then(|b| b.since_last_request()),
    }
}

/// One check; downloads a newer build. Returns when to check next.
async fn check(app: &AppHandle, pending: &mut Option<Pending>) -> Duration {
    if let Some(p) = pending.as_ref() {
        // Already downloaded: say so again rather than fetching it twice.
        let version = p.update.version.clone();
        set_phase(app, Phase::Ready { version }, true);
        return next_check_in(true);
    }
    set_phase(app, Phase::Checking, false);
    match find_and_download(app).await {
        Ok(Some(p)) => {
            let version = p.update.version.clone();
            *pending = Some(p);
            set_phase(app, Phase::Ready { version }, true);
            next_check_in(true)
        }
        Ok(None) => {
            set_phase(app, Phase::UpToDate, true);
            next_check_in(true)
        }
        Err(message) => {
            set_phase(app, Phase::Error { message }, true);
            next_check_in(false)
        }
    }
}

async fn find_and_download(app: &AppHandle) -> Result<Option<Pending>, String> {
    let (_, channel) = current(app);
    let url = channel.endpoint().parse().map_err(|e| format!("{e}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .and_then(|b| b.build())
        .map_err(|e| format!("Could not start the updater: {e}"))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("Could not check for updates: {e}"))?
    else {
        return Ok(None);
    };
    let version = update.version.clone();
    set_phase(
        app,
        Phase::Downloading {
            version: version.clone(),
            percent: None,
        },
        false,
    );
    let (mut got, mut shown) = (0u64, None);
    let bytes = update
        .download(
            |chunk, total| {
                got += chunk as u64;
                let p = percent(got, total);
                if p != shown {
                    shown = p;
                    let version = version.clone();
                    set_phase(
                        app,
                        Phase::Downloading {
                            version,
                            percent: p,
                        },
                        false,
                    );
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("Could not download {}: {e}", update.version))?;
    Ok(Some(Pending { update, bytes }))
}

/// Install and restart. Windows: the installer takes over and this process
/// exits inside `install`. On failure the download is dropped; the next check
/// fetches it again.
fn install(app: &AppHandle, p: Pending, silent: bool) {
    let version = p.update.version.clone();
    set_phase(app, Phase::Installing { version }, false);
    let marker = app
        .path()
        .app_data_dir()
        .ok()
        .map(|d| d.join(SILENT_RESTART_MARKER));
    if silent {
        if let Some(m) = &marker {
            let _ = std::fs::write(m, b"");
        }
    }
    match p.update.install(&p.bytes) {
        Ok(()) => app.restart(),
        Err(e) => {
            if let Some(m) = &marker {
                let _ = std::fs::remove_file(m);
            }
            set_phase(
                app,
                Phase::Error {
                    message: format!("Could not install {}: {e}", p.update.version),
                },
                false,
            );
        }
    }
}

/// `true` once if the last run installed an update on its own (and then the
/// relaunch stays in the tray). Removes the marker.
pub fn take_silent_restart(data_dir: Option<PathBuf>) -> bool {
    data_dir.is_some_and(|d| take_marker(&d.join(SILENT_RESTART_MARKER)))
}

pub fn take_marker(path: &Path) -> bool {
    std::fs::remove_file(path).is_ok()
}

#[tauri::command]
pub fn get_update_status(updates: tauri::State<'_, Updates>) -> UpdateStatus {
    updates.status()
}

#[tauri::command]
pub fn check_for_updates(updates: tauri::State<'_, Updates>) {
    updates.request(Request::Check);
}

#[tauri::command]
pub fn install_update(updates: tauri::State<'_, Updates>) {
    updates.request(Request::InstallNow);
}
