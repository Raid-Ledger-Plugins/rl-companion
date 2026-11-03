//! `GET /dev/v1/health` body (`dev-bridge/v1` `$defs/health`).

use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const MAX_FLAVORS: usize = 16;
const MAX_WOW_DIR: usize = 512;

pub struct HealthInput<'a> {
    pub wow_dir: &'a str,
    pub probes_enabled: bool,
    pub uptime: Duration,
    pub now_unix: i64,
}

pub fn health(input: &HealthInput<'_>) -> Value {
    let dir = input.wow_dir.trim();
    let wow_dir =
        (!dir.is_empty() && dir.chars().count() <= MAX_WOW_DIR).then(|| dir.replace('\\', "/"));
    let flavors = if dir.is_empty() {
        Vec::new()
    } else {
        discover_flavors(Path::new(dir))
    };
    json!({
        "app": "rl-companion",
        "appVersion": env!("CARGO_PKG_VERSION"),
        "bridgeVersion": 1,
        "devMode": true,
        "probesEnabled": input.probes_enabled,
        "wowDir": wow_dir,
        "flavors": flavors,
        // Process detection lands with the probe endpoints (C4).
        "wowRunning": false,
        "uptimeSec": input.uptime.as_secs(),
        "now": rl_collectors::rfc3339_utc_from_unix_seconds(input.now_unix),
    })
}

/// `^_[a-z0-9_]{1,32}_$` — the contract's flavor folder pattern.
pub fn is_flavor(name: &str) -> bool {
    let inner = name
        .strip_prefix('_')
        .and_then(|n| n.strip_suffix('_'))
        .unwrap_or("");
    (1..=32).contains(&inner.len())
        && inner
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Flavor folders directly under the WoW folder, sorted, at most 16.
pub fn discover_flavors(wow_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(wow_dir) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_flavor(n))
        .collect();
    found.sort();
    found.truncate(MAX_FLAVORS);
    found
}
