//! The version this build reports everywhere (`/dev/v1/health`, uploads,
//! linking, the HTTP user agent). A release stamps the tag's version into the
//! Tauri config only, so `Cargo.toml` stays `0.1.0`; the app sets the runtime
//! version (`package_info().version`) here at startup. Tests and code running
//! before setup fall back to the crate version.

use std::sync::OnceLock;

static RUNTIME: OnceLock<String> = OnceLock::new();

/// Record the runtime version once, at startup. Later calls are ignored.
pub fn set(version: String) {
    let _ = RUNTIME.set(version);
}

/// The runtime version, or the crate version before [`set`] ran.
pub fn get() -> &'static str {
    resolve(RUNTIME.get().map(String::as_str))
}

/// `runtime` when known and non-empty, else the crate version.
pub fn resolve(runtime: Option<&str>) -> &str {
    runtime
        .filter(|v| !v.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}
