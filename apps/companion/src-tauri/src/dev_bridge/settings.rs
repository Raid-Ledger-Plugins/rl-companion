//! Persisted dev-bridge settings (ROK-2055 §6.5). Stored inside `config.json`;
//! holds the dev token's sha256 only — never an `rlcdev_` value.
//!
//! Addresses are kept as strings so a hand-edited bad entry can't fail the
//! whole config load (which would reset every other setting); they are
//! validated when set and again when the listener starts.

use serde::{Deserialize, Serialize};

/// Spec §12 Q8: fixed default, changeable in the panel.
pub const DEFAULT_PORT: u16 = 47655;

/// Most allowed callers the panel accepts.
pub const MAX_ALLOWLIST: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DevBridgeConfig {
    /// The Developer bridge toggle. Off by default (AC-2).
    pub enabled: bool,
    /// The one RFC1918 IPv4 the listener binds; `None` = not chosen yet.
    pub bind_ip: Option<String>,
    pub port: u16,
    /// Remote IPv4s allowed to call; empty = refuse everyone.
    pub allowlist: Vec<String>,
    /// "Allow probes" (C4 consumes it; reported by `/dev/v1/health`).
    pub probes_enabled: bool,
    /// Lowercase hex sha256 of the current dev token; `None` = no token yet.
    pub token_sha256: Option<String>,
}

impl Default for DevBridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_ip: None,
            port: DEFAULT_PORT,
            allowlist: Vec::new(),
            probes_enabled: false,
            token_sha256: None,
        }
    }
}

impl DevBridgeConfig {
    /// Untouched settings are left out of `config.json`.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
