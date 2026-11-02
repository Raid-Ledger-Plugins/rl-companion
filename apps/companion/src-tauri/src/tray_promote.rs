//! First-run tray promotion on Windows 11 (ROK-2065).
//!
//! Windows 11 22H2+ hides new tray icons in the overflow, and with the hidden
//! icon menu turned off the Companion is simply invisible. Explorer records
//! each icon under `HKCU\Control Panel\NotifyIconSettings\<id>` (created only
//! after the icon first appears) with `ExecutablePath` and `IsPromoted`.
//!
//! At the operator's request this deliberately writes that user-preference key
//! ONCE: on first run it sets `IsPromoted = 1` for our executable and records
//! `trayPromotedOnce` in config.json. It never re-promotes, so a user who later
//! hides the icon keeps it hidden. Failures are logged, never fatal.
//!
//! The decisions are pure and tested everywhere; only the registry I/O is
//! `#[cfg(windows)]`.

use tauri::AppHandle;

/// Explorer's parent key (HKCU). Missing on Windows 10 and older 11 builds.
pub const NOTIFY_ICON_SETTINGS: &str = r"Control Panel\NotifyIconSettings";
/// Retry cadence while waiting for Explorer to create our key.
pub const RETRY_EVERY_SECS: u64 = 2;
pub const RETRY_ATTEMPTS: u32 = 30;

/// Known-folder ids Explorer may write in place of a path prefix, paired with
/// the environment variable that resolves each one.
pub const KNOWN_FOLDERS: &[(&str, &str)] = &[
    ("{6D809377-6AF0-444B-8957-A3773F02200E}", "ProgramFiles"),
    (
        "{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}",
        "ProgramFiles(x86)",
    ),
    ("{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}", "LOCALAPPDATA"),
];

/// Whether to start the promotion retry at all: only until it succeeded once.
pub fn should_promote(promoted_once: bool) -> bool {
    !promoted_once
}

/// Expand a leading known-folder id (`folders` = id → resolved directory),
/// then compare-ready: `\\?\` stripped, `/` → `\`, trailing `\` trimmed,
/// lower-cased (Windows paths are case-insensitive).
pub fn normalise_path(path: &str, folders: &[(String, String)]) -> String {
    let mut p = path.trim().to_owned();
    for (id, dir) in folders {
        let head = p.get(..id.len());
        if head.is_some_and(|h| h.eq_ignore_ascii_case(id)) {
            p = format!("{}{}", dir.trim_end_matches(['\\', '/']), &p[id.len()..]);
            break;
        }
    }
    let p = p.strip_prefix(r"\\?\").unwrap_or(&p).replace('/', "\\");
    p.trim_end_matches('\\').to_lowercase()
}

/// Same file, ignoring case, separators and known-folder ids.
pub fn paths_match(stored: &str, exe: &str, folders: &[(String, String)]) -> bool {
    let a = normalise_path(stored, folders);
    !a.is_empty() && a == normalise_path(exe, folders)
}

/// The subkey (of `(subkey, ExecutablePath)` pairs) that belongs to `exe`.
pub fn find_entry<'a>(
    entries: &'a [(String, String)],
    exe: &str,
    folders: &[(String, String)],
) -> Option<&'a str> {
    entries
        .iter()
        .find(|(_, stored)| paths_match(stored, exe, folders))
        .map(|(key, _)| key.as_str())
}

/// Start the once-only promotion in the background (Windows); no-op elsewhere.
pub fn start(app: &AppHandle) {
    #[cfg(windows)]
    registry::start(app.clone());
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
mod registry {
    use super::*;
    use crate::state::Shared;
    use tauri::Manager;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    use winreg::RegKey;

    enum Outcome {
        Promoted,
        NoParentKey,
        NotYet,
    }

    pub fn start(app: AppHandle) {
        if !should_promote(app.state::<Shared>().config().tray_promoted_once) {
            return;
        }
        std::thread::spawn(move || run(app));
    }

    fn run(app: AppHandle) {
        let exe = match std::env::current_exe() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                eprintln!("RL Companion: tray promotion: no exe path: {e}");
                return;
            }
        };
        let folders = resolved_folders();
        let mut logged_error = false;
        for _ in 0..RETRY_ATTEMPTS {
            match try_promote(&exe, &folders) {
                Ok(Outcome::Promoted) => {
                    let saved = app
                        .state::<Shared>()
                        .update_config(|c| c.tray_promoted_once = true);
                    if let Err(e) = saved {
                        eprintln!("RL Companion: tray promotion: could not save flag: {e}");
                    }
                    return;
                }
                Ok(Outcome::NoParentKey) => return,
                Ok(Outcome::NotYet) => {}
                // Log the first failure only: the retry loop runs up to 30 times.
                Err(e) if !logged_error => {
                    logged_error = true;
                    eprintln!("RL Companion: tray promotion: {e}");
                }
                Err(_) => {}
            }
            std::thread::sleep(std::time::Duration::from_secs(RETRY_EVERY_SECS));
        }
        eprintln!("RL Companion: tray promotion: no Explorer entry yet; retrying next start");
    }

    fn resolved_folders() -> Vec<(String, String)> {
        let resolve = |(id, var): &(&str, &str)| Some(((*id).to_owned(), std::env::var(var).ok()?));
        KNOWN_FOLDERS.iter().filter_map(resolve).collect()
    }

    fn try_promote(exe: &str, folders: &[(String, String)]) -> std::io::Result<Outcome> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(parent) = hkcu.open_subkey_with_flags(NOTIFY_ICON_SETTINGS, KEY_READ) else {
            return Ok(Outcome::NoParentKey);
        };
        let entries: Vec<(String, String)> = parent
            .enum_keys()
            .filter_map(Result::ok)
            .filter_map(|name| {
                let key = parent.open_subkey_with_flags(&name, KEY_READ).ok()?;
                let path: String = key.get_value("ExecutablePath").ok()?;
                Some((name, path))
            })
            .collect();
        let Some(name) = find_entry(&entries, exe, folders) else {
            return Ok(Outcome::NotYet);
        };
        let key = parent.open_subkey_with_flags(name, KEY_READ | KEY_SET_VALUE)?;
        key.set_value("IsPromoted", &1u32)?;
        Ok(Outcome::Promoted)
    }
}
