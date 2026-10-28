//! App settings persisted as JSON in the OS app-config dir. Never holds the token.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Production Raid Ledger. Overridable to a fleet `https://slot-N.gamernight.net`.
pub const DEFAULT_SERVER_URL: &str = "https://raid.gamernight.net";

/// The api is served under `/api/` behind the site's nginx.
pub const API_PREFIX: &str = "api/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    /// Normalised origin, e.g. `https://raid.gamernight.net` (no trailing slash).
    pub server_url: String,
    /// User-chosen device name; `None` = the hostname.
    pub device_name: Option<String>,
    /// WoW install folder (the one holding `_classic_era_`/`_retail_`); the
    /// window's folder picker (C-3) changes it.
    pub wow_dir: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_owned(),
            device_name: None,
            wow_dir: default_wow_dir().to_owned(),
        }
    }
}

impl AppConfig {
    /// Missing or unreadable file → defaults (first run). A bad stored URL also
    /// falls back to the default rather than bricking start-up.
    pub fn load(path: &Path) -> Self {
        let mut cfg: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        cfg.server_url =
            normalise_server_url(&cfg.server_url).unwrap_or_else(|_| DEFAULT_SERVER_URL.to_owned());
        cfg
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

/// Reduce user input to a bare origin. `https` only, except plain `http` for
/// `localhost` / `127.0.0.1` (local dev). No path, query, fragment or userinfo.
pub fn normalise_server_url(input: &str) -> Result<String, String> {
    let url = url::Url::parse(input.trim()).map_err(|e| format!("Not a valid URL: {e}"))?;
    let host = url.host_str().ok_or("The URL has no host")?.to_owned();
    let local = host == "localhost" || host == "127.0.0.1";
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        _ => return Err("The server URL must start with https://".into()),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("The server URL must not contain a username or password".into());
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err("Enter just the site address, e.g. https://raid.gamernight.net".into());
    }
    Ok(url.origin().ascii_serialization())
}

/// Display-safe device name (CONTRACT.md §6): no control, bidi or line/para
/// separator chars, trimmed, 1..=64 chars. `None` if nothing usable is left.
pub fn sanitise_device_name(input: &str) -> Option<String> {
    let cleaned: String = input
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let name: String = cleaned.trim().chars().take(64).collect();
    let name = name.trim_end().to_owned();
    (!name.is_empty()).then_some(name)
}

/// Default device name: the hostname (spike §5 — no OS username or hardware id).
pub fn default_device_name() -> String {
    sanitise_device_name(&gethostname::gethostname().to_string_lossy())
        .unwrap_or_else(|| "My computer".to_owned())
}

/// Default WoW folder per OS. Day 1 runs on Windows (operator ruling 2026-10-28).
pub fn default_wow_dir() -> &'static str {
    if cfg!(windows) {
        r"C:\Program Files (x86)\World of Warcraft"
    } else if cfg!(target_os = "macos") {
        "/Applications/World of Warcraft"
    } else {
        ""
    }
}

/// The contract's `device.os` value for this build.
pub fn device_os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        _ => "other",
    }
}
