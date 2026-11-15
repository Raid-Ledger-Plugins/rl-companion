//! Which flavor folder a probe is queued in. An explicit flavor must be a
//! discovered one; `auto` is the flavor whose account-scope
//! `LedgerLink.lua` SavedVariables was written most recently.

use super::health::discover_flavors;
use super::probe_request::ProbeRequest;
use super::sv::sv_files;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlavorError {
    #[error("flavor {0} is not a folder in the WoW folder")]
    Unknown(String),
    #[error("no flavor has LedgerLink SavedVariables yet — log in once with LedgerLink enabled, or name a flavor")]
    NoneSeen,
}

/// Newest mtime of `flavor`'s account-scope LedgerLink.lua files.
pub fn last_seen(wow_dir: &Path, flavor: &str) -> Option<SystemTime> {
    sv_files(wow_dir, flavor)
        .into_iter()
        .filter(|f| f.scope == "account")
        .filter_map(|f| std::fs::metadata(&f.abs).and_then(|m| m.modified()).ok())
        .max()
}

/// The folder `req` is written to (never `auto`).
pub fn resolve_flavor(wow_dir: &Path, req: &ProbeRequest) -> Result<String, FlavorError> {
    let found = discover_flavors(wow_dir);
    if !req.is_auto_flavor() {
        return found
            .contains(&req.flavor)
            .then(|| req.flavor.clone())
            .ok_or_else(|| FlavorError::Unknown(req.flavor.clone()));
    }
    found
        .into_iter()
        .filter_map(|f| last_seen(wow_dir, &f).map(|t| (t, f)))
        .max()
        .map(|(_, f)| f)
        .ok_or(FlavorError::NoneSeen)
}
