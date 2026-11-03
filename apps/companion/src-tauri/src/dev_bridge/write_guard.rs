//! The one function the dev bridge writes files through (spec §7, AC-13).
//! Only `<wowDir>/<flavor>/Interface/AddOns/LedgerLinkProbe/…` is writable —
//! `Queue.lua` and the bundled probe-addon files (C4). Every other target is
//! refused before anything touches the disk.

use super::health::is_flavor;
use std::io::Write;
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

/// Write `bytes` to `target` atomically (temp file + rename). Every folder
/// from `wow_dir` down is checked BEFORE anything is created: a symlinked or
/// redirected step is refused, and the resolved parent must sit inside the
/// flavor's probe addon folder. The temp file is opened `create_new`, so a
/// link planted at its name is never followed.
pub fn guarded_write(wow_dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), WriteGuardError> {
    assert_allowed(wow_dir, target)?;
    let outside = || WriteGuardError::Outside(target.to_path_buf());
    let parent = target.parent().ok_or_else(outside)?;
    let rel = parent.strip_prefix(wow_dir).map_err(|_| outside())?;
    let real_root = std::fs::canonicalize(wow_dir)?;
    let (mut cur, mut expect) = (wow_dir.to_path_buf(), real_root.clone());
    for part in rel.components() {
        cur.push(part);
        expect.push(part);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => return Err(outside()),
            Ok(_) if !same_path(&std::fs::canonicalize(&cur)?, &expect) => return Err(outside()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&cur)?,
            Err(e) => return Err(e.into()),
        }
    }
    let flavor = rel.components().next().ok_or_else(outside)?;
    let probe_root = probe_addon_dir(&real_root, &flavor.as_os_str().to_string_lossy())?;
    if !path_inside(&std::fs::canonicalize(parent)?, &probe_root) {
        return Err(outside());
    }
    Ok(write_new_then_rename(target, bytes, None)?)
}

/// Path equality as the filesystem sees it: Windows paths are
/// case-insensitive, so an install's `Interface\Addons` matches the
/// `Interface/AddOns` this module builds (review MINOR-D4).
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        lower_parts(a) == lower_parts(b)
    } else {
        a == b
    }
}

/// `child` is `root` or under it, compared like [`same_path`].
pub fn path_inside(child: &Path, root: &Path) -> bool {
    if cfg!(windows) {
        lower_parts(child).starts_with(&lower_parts(root))
    } else {
        child.starts_with(root)
    }
}

/// A path's components, lower-cased (the case-insensitive compare).
pub fn lower_parts(p: &Path) -> Vec<String> {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// Atomic write of a secret (the TLS key): owner-only (0600) on unix.
pub fn write_private(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_new_then_rename(target, bytes, Some(0o600))
}

/// Write `<target>.tmp` (a fresh file — an existing one, or a link planted
/// there, is removed rather than followed), then rename it over `target`.
fn write_new_then_rename(target: &Path, bytes: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    let tmp = crate::config::tmp_path(target);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(m);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let written = opts.open(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    let renamed = written.and_then(|()| std::fs::rename(&tmp, target));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    renamed
}

/// The dev snapshot cache file (ROK-2072 §6): `<root>/<flavor>/<key>.jsonl`,
/// where `flavor` is a contract flavor folder name and `key` 64 lower hex.
/// Neither ever comes from request text unchecked.
pub fn cache_file(root: &Path, flavor: &str, key: &str) -> Result<PathBuf, WriteGuardError> {
    if !is_flavor(flavor) {
        return Err(WriteGuardError::BadFlavor(flavor.to_owned()));
    }
    let hex = key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let target = root.join(flavor).join(format!("{key}.jsonl"));
    if hex {
        Ok(target)
    } else {
        Err(WriteGuardError::Outside(target))
    }
}

/// Write one cache file atomically. The flavor folder under `root` is
/// created if missing and refused if it is a link or not a folder.
pub fn guarded_cache_write(
    root: &Path,
    flavor: &str,
    key: &str,
    bytes: &[u8],
) -> Result<(), WriteGuardError> {
    let target = cache_file(root, flavor, key)?;
    std::fs::create_dir_all(root)?;
    let dir = root.join(flavor);
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(WriteGuardError::Outside(dir))
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&dir)?,
        Err(e) => return Err(e.into()),
    }
    Ok(write_new_then_rename(&target, bytes, None)?)
}

/// Delete one cache file (an emptied history); a missing file is fine.
pub fn guarded_cache_remove(root: &Path, flavor: &str, key: &str) -> Result<(), WriteGuardError> {
    match std::fs::remove_file(cache_file(root, flavor, key)?) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}
