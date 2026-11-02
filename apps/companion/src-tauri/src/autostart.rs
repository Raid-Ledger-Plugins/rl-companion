//! Start at login (ROK-2065) and the launch-visibility rules that go with it.
//!
//! The OS starts the app with [`AUTOSTART_ARG`]; that launch stays in the tray.
//! Start-at-login is turned on once, on the first run that sees
//! `autostart_initialized == false`; after that the user's choice (the window's
//! "Start at login" switch) is never overridden, across restarts and updates.

use crate::state::Shared;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

/// The argument the OS login item passes. Registered with the autostart plugin.
pub const AUTOSTART_ARG: &str = "--autostart";

/// True when `args` (a process argv, program path included) carry [`AUTOSTART_ARG`].
pub fn launched_by_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|a| a.as_ref() == AUTOSTART_ARG)
}

/// Whether a launch with `args` should show the main window. Used for this
/// process's own start and for a second launch the single-instance guard
/// forwards to us: a login-item launch stays in the tray, anything else shows it.
pub fn should_show_window<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    !launched_by_autostart(args)
}

/// This process's argv as strings (lossy, so a non-UTF-8 path can't panic).
pub fn process_args() -> Vec<String> {
    std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// What the first-run check does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstRun {
    /// Already decided on an earlier run: leave the OS login item alone.
    Nothing,
    /// First run and start-at-login is off: turn it on, then record that.
    EnableAndMark,
    /// First run but it is already on (e.g. a reinstall): only record it.
    MarkOnly,
}

/// `initialized` = the config's `autostart_initialized`; `enabled` = the OS
/// login item's current state.
pub fn first_run_action(initialized: bool, enabled: bool) -> FirstRun {
    match (initialized, enabled) {
        (true, _) => FirstRun::Nothing,
        (false, false) => FirstRun::EnableAndMark,
        (false, true) => FirstRun::MarkOnly,
    }
}

/// Run once from `setup`: default start-at-login to on, exactly once. A failed
/// `enable()` leaves the flag unset so the next run tries again.
pub fn init_default(app: &AppHandle) {
    let shared = app.state::<Shared>();
    let launcher = app.autolaunch();
    let enabled = launcher.is_enabled().unwrap_or(false);
    match first_run_action(shared.config().autostart_initialized, enabled) {
        FirstRun::Nothing => return,
        FirstRun::EnableAndMark => {
            if let Err(e) = launcher.enable() {
                eprintln!("RL Companion: could not turn on start at login: {e}");
                return;
            }
        }
        FirstRun::MarkOnly => {}
    }
    if let Err(e) = shared.update_config(|c| c.autostart_initialized = true) {
        eprintln!("RL Companion: could not save the start-at-login flag: {e}");
    }
}
