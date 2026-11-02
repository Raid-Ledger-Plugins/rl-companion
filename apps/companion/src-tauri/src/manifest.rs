//! Collector manifests (ROK-2061, spike ROK-1951 §3.3): the data half of a
//! collector. The code (watch + parse) is compiled in; the manifest ships in
//! the signed build too, so there are no external manifest files.

use std::path::PathBuf;

/// The OS a default watch root applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    /// The platform this build runs on (`None` on anything else).
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "windows" => Some(Self::Windows),
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            _ => None,
        }
    }
}

/// One default watch root. `path` may start with `~` or hold `%VAR%`
/// (Windows-style env vars), resolved at runtime by [`resolve_root`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootSpec {
    pub platform: Platform,
    pub path: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectorManifest {
    /// Stable id, also the key in the settings' enabled map.
    pub id: &'static str,
    /// Name the window shows ("turn on {display_name} uploads").
    pub display_name: &'static str,
    /// Raid Ledger `COMPANION_CONTRACTS` id + version this collector emits.
    pub contract_id: &'static str,
    pub contract_version: u32,
    /// Opt-in: collectors start off unless the manifest says otherwise.
    pub default_enabled: bool,
    /// Default roots per OS; the user can override them in the settings.
    pub default_roots: &'static [RootSpec],
    /// File-name patterns the collector reads (`*` wildcard, any case).
    pub file_patterns: &'static [&'static str],
    /// How long a changed file must stay unchanged before it is read.
    pub settle_ms: u64,
    /// Status line when the scan found no source file at all.
    pub no_files_hint: &'static str,
}

/// Settle bounds: under 1 s the 1 s sample tick cannot see it; over a minute
/// the user would think the app is stuck.
pub const SETTLE_MS_RANGE: std::ops::RangeInclusive<u64> = 1_000..=60_000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("collector id {0:?} must be lowercase letters, digits and dashes")]
    BadId(String),
    #[error("{0}: contract id {1:?} must be lowercase letters, digits and dashes")]
    BadContract(&'static str, &'static str),
    #[error("{0}: contract version must be at least 1")]
    BadVersion(&'static str),
    #[error("{0}: display name is empty")]
    NoName(&'static str),
    #[error("{0}: needs at least one file pattern")]
    NoPatterns(&'static str),
    #[error("{0}: a default root is empty")]
    EmptyRoot(&'static str),
    #[error("{0}: settle delay {1} ms is outside 1000..=60000")]
    BadSettle(&'static str, u64),
    #[error("collector id {0:?} is declared twice")]
    Duplicate(&'static str),
}

fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl CollectorManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        let id = self.id;
        if !is_slug(id) {
            return Err(ManifestError::BadId(id.to_owned()));
        }
        if !is_slug(self.contract_id) {
            return Err(ManifestError::BadContract(id, self.contract_id));
        }
        if self.contract_version == 0 {
            return Err(ManifestError::BadVersion(id));
        }
        if self.display_name.trim().is_empty() {
            return Err(ManifestError::NoName(id));
        }
        if self.file_patterns.iter().all(|p| p.trim().is_empty()) {
            return Err(ManifestError::NoPatterns(id));
        }
        if self.default_roots.iter().any(|r| r.path.trim().is_empty()) {
            return Err(ManifestError::EmptyRoot(id));
        }
        if !SETTLE_MS_RANGE.contains(&self.settle_ms) {
            return Err(ManifestError::BadSettle(id, self.settle_ms));
        }
        Ok(())
    }

    /// Settle samples on the runtime's 1 s tick (rounded up).
    pub fn settle_samples(&self) -> usize {
        usize::try_from(self.settle_ms.div_ceil(1_000)).unwrap_or(usize::MAX)
    }

    /// Whether a file name matches one of the patterns (case-insensitive).
    pub fn matches_file(&self, name: &str) -> bool {
        self.file_patterns.iter().any(|p| glob_match(p, name))
    }

    /// This OS's default roots, resolved with `env`.
    pub fn default_roots_for(
        &self,
        platform: Option<Platform>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Vec<PathBuf> {
        self.default_roots
            .iter()
            .filter(|r| Some(r.platform) == platform)
            .filter_map(|r| resolve_root(r.path, env))
            .collect()
    }
}

/// Validate a set: each manifest valid, ids unique.
pub fn validate_all(manifests: &[&CollectorManifest]) -> Result<(), ManifestError> {
    let mut seen: Vec<&str> = Vec::new();
    for m in manifests {
        m.validate()?;
        if seen.contains(&m.id) {
            return Err(ManifestError::Duplicate(m.id));
        }
        seen.push(m.id);
    }
    Ok(())
}

/// Expand a leading `~` (HOME / USERPROFILE) and `%VAR%` segments. `None` when
/// a variable is unset, so a root that cannot exist is dropped, not guessed.
pub fn resolve_root(template: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let mut out = String::new();
    let mut rest = template;
    // Only `~` alone or `~/…` / `~\…`: `~foo` (another user's home) is literal.
    let home_tail = rest
        .strip_prefix('~')
        .filter(|t| t.is_empty() || t.starts_with('/') || t.starts_with('\\'));
    if let Some(tail) = home_tail {
        out.push_str(&env("HOME").or_else(|| env("USERPROFILE"))?);
        rest = tail;
    }
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        out.push_str(&rest[..start]);
        out.push_str(&env(&after[..end])?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(PathBuf::from(out))
}

/// A user root override (settings): `~` and `%VAR%` expanded like a manifest
/// root, then it must be absolute and not a bare filesystem root (`/`, `C:\`),
/// which would make the collector's scope meaningless.
pub fn resolve_override(
    raw: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<PathBuf, String> {
    let raw = raw.trim();
    let path = resolve_root(raw, env)
        .ok_or_else(|| format!("folder {raw:?} names an environment variable that is not set"))?;
    if !path.is_absolute() {
        return Err(format!("folder {raw:?} must be a full path"));
    }
    if path.parent().is_none() {
        return Err(format!(
            "folder {raw:?} is a whole drive; pick the game's folder"
        ));
    }
    Ok(path)
}

/// `*` matches any run of characters; everything else literally, any case.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    let parts: Vec<&str> = p.split('*').collect();
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else {
        return false;
    };
    if parts.len() == 1 {
        return p == n;
    }
    if !n.starts_with(first) || n.len() < first.len() + last.len() || !n.ends_with(last) {
        return false;
    }
    let mut pos = first.len();
    let end = n.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match n[pos..end].find(mid) {
            Some(i) => pos += i + mid.len(),
            None => return false,
        }
    }
    true
}

/// The WoW LedgerLink collector (ROK-1964). Its root is the WoW install folder;
/// the settings' `wowDir` is the user override.
pub const WOW_LEDGERLINK: CollectorManifest = CollectorManifest {
    id: "wow-ledgerlink",
    display_name: "LedgerLink",
    contract_id: "wow-ledgerlink",
    contract_version: 1,
    default_enabled: false,
    default_roots: &[
        RootSpec {
            platform: Platform::Windows,
            path: r"C:\Program Files (x86)\World of Warcraft",
        },
        RootSpec {
            platform: Platform::Macos,
            path: "/Applications/World of Warcraft",
        },
    ],
    file_patterns: &["LedgerLink.lua"],
    settle_ms: 3_000,
    no_files_hint: "No LedgerLink.lua found — check the WoW folder",
};
