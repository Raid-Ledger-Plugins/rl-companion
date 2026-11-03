//! The one function the dev bridge writes files through (spec §7, AC-13).
//! Only `<wowDir>/<flavor>/Interface/AddOns/LedgerLinkProbe/…` is writable —
//! `Queue.lua` and the bundled probe-addon files (C4). Every other target is
//! refused before anything touches the disk.

use super::health::is_flavor;
use std::path::{Component, Path, PathBuf};

pub const PROBE_ADDON: &str = "LedgerLinkProbe";
pub const QUEUE_FILE: &str = "Queue.lua";

#[derive(Debug, thiserror::Error)]
pub enum WriteGuardError {
    #[error("not a WoW flavor folder: {0}")]
    BadFlavor(String),
    #[error("refused to write outside the probe addon folder: {0}")]
    Outside(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// `<wowDir>/<flavor>/Interface/AddOns/LedgerLinkProbe`.
pub fn probe_addon_dir(wow_dir: &Path, flavor: &str) -> Result<PathBuf, WriteGuardError> {
    if !is_flavor(flavor) {
        return Err(WriteGuardError::BadFlavor(flavor.to_owned()));
    }
    Ok(wow_dir
        .join(flavor)
        .join("Interface")
        .join("AddOns")
        .join(PROBE_ADDON))
}

pub fn queue_path(wow_dir: &Path, flavor: &str) -> Result<PathBuf, WriteGuardError> {
    Ok(probe_addon_dir(wow_dir, flavor)?.join(QUEUE_FILE))
}

/// Lexical check: `target` is a file strictly inside a flavor's probe addon
/// folder, reached through plain path segments only (no `..`, roots, drives).
pub fn assert_allowed(wow_dir: &Path, target: &Path) -> Result<(), WriteGuardError> {
    let outside = || WriteGuardError::Outside(target.to_path_buf());
    let rel = target.strip_prefix(wow_dir).map_err(|_| outside())?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_str().ok_or_else(outside)?),
            _ => return Err(outside()),
        }
    }
    let ok = parts.len() >= 5
        && is_flavor(parts[0])
        && parts[1] == "Interface"
        && parts[2] == "AddOns"
        && parts[3] == PROBE_ADDON;
    if ok {
        Ok(())
    } else {
        Err(outside())
    }
}

/// Write `bytes` to `target` atomically (temp file + rename), after
/// [`assert_allowed`] and a symlink check on the resolved parent folder.
pub fn guarded_write(wow_dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), WriteGuardError> {
    assert_allowed(wow_dir, target)?;
    let parent = target
        .parent()
        .ok_or_else(|| WriteGuardError::Outside(target.into()))?;
    std::fs::create_dir_all(parent)?;
    let real_parent = std::fs::canonicalize(parent)?;
    let real_root = std::fs::canonicalize(wow_dir)?;
    if !real_parent.starts_with(&real_root) {
        return Err(WriteGuardError::Outside(target.into()));
    }
    let tmp = crate::config::tmp_path(target);
    let written = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, target));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(written?)
}
