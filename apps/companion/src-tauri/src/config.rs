//! App settings persisted as JSON in the OS app-config dir. Never holds the token.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    /// Per-collector upload switch, keyed by manifest id (ROK-2061). A missing
    /// id means the manifest's default (off: turning it on in the window is
    /// the first-upload confirmation).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub collectors_enabled: BTreeMap<String, bool>,
    /// Per-collector root overrides, keyed by manifest id. WoW keeps using
    /// `wow_dir`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub collector_roots: BTreeMap<String, Vec<String>>,
    /// Collectors whose first-upload preview the user confirmed (ROK-2061).
    /// Later uploads go without asking.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub collector_consent: BTreeMap<String, bool>,
    /// Pre-ROK-2061 LedgerLink switch. Read only, folded into
    /// `collectors_enabled` by [`AppConfig::load`], never written back.
    #[serde(rename = "ledgerlinkEnabled", skip_serializing)]
    pub legacy_ledgerlink_enabled: Option<bool>,
    /// Start-at-login was defaulted on once (ROK-2065). After that the user's
    /// choice stands; the app never turns it back on. Missing = first run.
    pub autostart_initialized: bool,
}

/// The id the legacy `ledgerlinkEnabled` flag belongs to.
pub const LEGACY_LEDGERLINK_ID: &str = crate::manifest::WOW_LEDGERLINK.id;

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_owned(),
            device_name: None,
            wow_dir: default_wow_dir().to_owned(),
            collectors_enabled: BTreeMap::new(),
            collector_roots: BTreeMap::new(),
            collector_consent: BTreeMap::new(),
            legacy_ledgerlink_enabled: None,
            autostart_initialized: false,
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
        cfg.migrate();
        cfg
    }

    /// Fold the legacy `ledgerlinkEnabled` flag into `collectors_enabled`. An
    /// explicit per-collector entry wins over the legacy flag.
    pub fn migrate(&mut self) {
        if let Some(on) = self.legacy_ledgerlink_enabled.take() {
            self.collectors_enabled
                .entry(LEGACY_LEDGERLINK_ID.to_owned())
                .or_insert(on);
        }
    }

    /// The user's switch for `id`, or `default` when never set.
    pub fn collector_enabled(&self, id: &str, default: bool) -> bool {
        self.collectors_enabled.get(id).copied().unwrap_or(default)
    }

    pub fn set_collector_enabled(&mut self, id: &str, on: bool) {
        self.collectors_enabled.insert(id.to_owned(), on);
    }

    /// The user confirmed `id`'s first-upload preview.
    pub fn consented(&self, id: &str) -> bool {
        self.collector_consent.get(id).copied().unwrap_or(false)
    }

    pub fn set_consented(&mut self, id: &str) {
        self.collector_consent.insert(id.to_owned(), true);
    }

    /// The user's root override for `id`, if any (empty entries ignored).
    pub fn roots_override(&self, id: &str) -> Option<Vec<String>> {
        if id == LEGACY_LEDGERLINK_ID {
            let dir = self.wow_dir.trim();
            return (!dir.is_empty()).then(|| vec![dir.to_owned()]);
        }
        let roots: Vec<String> = self
            .collector_roots
            .get(id)?
            .iter()
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty())
            .collect();
        (!roots.is_empty()).then_some(roots)
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
