//! `bg3-saves` rows (ROK-2078 layer 1): one per `.lsv` under the BG3
//! `Savegames/Story` roots, identified only by `saveRef` (HMAC of the
//! save-relative path under a per-install secret). Reads are confined to the
//! roots by [`WatchScope`]; nothing is ingested — every read is on request.

use super::bg3_newage::{NewAgeDecoder, Readability};
use super::bg3_redact::game_version;
use crate::bg3_collector::MAX_LSV_BYTES;
use crate::collector::captured_at;
use crate::manifest::BG3_SAVE;
use crate::scope::WatchScope;
use rl_collectors::{
    campaign_key_from_package, extract_save_info, lsf_attribute_len, lsf_parsed_version,
    read_lsf_attribute, read_lspk, Package, META_LSF_MAX_BYTES, META_LSF_NAME,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Where the per-install `saveRef` secret lives under the app data dir.
pub const KEY_FILE: [&str; 2] = ["dev-bridge", "bg3-saveref.key"];
pub const GLOBALS_LSF_NAME: &str = "Globals.lsf";
/// Cap on the decompressed `Globals.lsf` (real: 3.9 MB).
pub const GLOBALS_MAX_BYTES: u64 = 64 * 1024 * 1024;
const NEWAGE_ATTRIBUTE: &str = "NewAge";

pub struct Bg3Source {
    roots: Vec<PathBuf>,
    scope: WatchScope,
    key_dir: Option<PathBuf>,
    key: OnceLock<Option<[u8; 32]>>,
    pub decoder: Arc<dyn NewAgeDecoder>,
}

/// One opened save: its listing row plus what the structure read needs.
pub struct Opened {
    pub row: Value,
    pub globals: Option<Vec<u8>>,
    pub party_size: usize,
}

/// RFC 2104 HMAC-SHA256 (sha2 only; no extra dependency).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.map(|x| x ^ b);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(msg)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

/// `saveRef`: the first 12 lowercase hex chars of
/// HMAC-SHA256(key, `<root>` NUL `<rel>`). The configured root is the
/// namespace, so the same folder/file pair under two roots gets two refs
/// (Codex P2); it is inside the keyed MAC, so it never reaches a body.
pub fn save_ref(key: &[u8], root: &Path, rel: &str) -> String {
    let msg = [root.to_string_lossy().as_bytes(), b"\0", rel.as_bytes()].concat();
    hmac_sha256(key, &msg)
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The persisted secret, created on first use; in-memory when `dir` is unset
/// or unwritable (refs then change per run, which only restarts paging).
/// `None` when the OS RNG fails: an all-zero key would make every `saveRef`
/// computable from guessable folder names, so the routes refuse instead.
fn load_or_create_key(dir: Option<&Path>) -> Option<[u8; 32]> {
    load_or_create_key_with(dir, |k| getrandom::getrandom(k).is_ok())
}

/// [`load_or_create_key`] with the RNG injected (`fill` → `false` = failed).
pub(crate) fn load_or_create_key_with(
    dir: Option<&Path>,
    fill: impl FnOnce(&mut [u8]) -> bool,
) -> Option<[u8; 32]> {
    let path = dir.map(|d| KEY_FILE.iter().fold(d.to_path_buf(), |p, s| p.join(s)));
    if let Some(key) = path.as_deref().and_then(read_key) {
        return Some(key);
    }
    let mut key = [0u8; 32];
    if !fill(&mut key) {
        super::logbuf::log(
            "warn",
            "dev_bridge::bg3",
            "no OS randomness for the saveRef key; bg3 routes refused",
        );
        return None;
    }
    if let Some(p) = path {
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let _ = p.parent().map(std::fs::create_dir_all);
        // Atomic (temp + rename) and 0600 on unix; on failure the key stays
        // in memory for this run.
        let _ = super::write_guard::write_private(&p, hex.as_bytes());
    }
    Some(key)
}

/// A stored key: 64 hex chars (the file is read at most 128 bytes).
fn read_key(path: &Path) -> Option<[u8; 32]> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(128)
        .read_to_string(&mut text)
        .ok()?;
    let t = text.trim();
    let bytes: Option<Vec<u8>> = (0..t.len())
        .step_by(2)
        .map(|i| t.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect();
    bytes?.try_into().ok()
}

impl Bg3Source {
    pub fn new(
        roots: Vec<PathBuf>,
        key_dir: Option<PathBuf>,
        decoder: Arc<dyn NewAgeDecoder>,
    ) -> Self {
        let scope = WatchScope::new(&roots);
        Self {
            roots,
            scope,
            key_dir,
            key: OnceLock::new(),
            decoder,
        }
    }

    /// A source with a fixed key (tests).
    pub fn with_key(roots: Vec<PathBuf>, key: [u8; 32], decoder: Arc<dyn NewAgeDecoder>) -> Self {
        let src = Self::new(roots, None, decoder);
        let _ = src.key.set(Some(key));
        src
    }

    /// The configured save roots (server-side only; never in a body).
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The `saveRef` secret; `None` when it could not be generated.
    pub fn key(&self) -> Option<&[u8; 32]> {
        self.key
            .get_or_init(|| load_or_create_key(self.key_dir.as_deref()))
            .as_ref()
    }

    /// `None` only when [`Self::key`] is unavailable (the routes refuse first).
    /// A source whose key generation failed (tests).
    #[cfg(test)]
    pub fn without_key(roots: Vec<PathBuf>, decoder: Arc<dyn NewAgeDecoder>) -> Self {
        let src = Self::new(roots, None, decoder);
        let _ = src.key.set(None);
        src
    }

    /// The ref of `abs` (`<root>/<folder>/<file>`, as [`Self::save_files`]
    /// lists it); its root is the namespace.
    pub fn save_ref(&self, abs: &Path, rel: &str) -> Option<String> {
        let root = abs.parent()?.parent()?;
        self.key().map(|k| save_ref(k, root, rel))
    }

    /// Every `<root>/<SaveFolder>/<*.lsv>` as `(absolute, "<folder>/<file>")`,
    /// sorted by the relative path (then the absolute one) so offsets are
    /// stable between pages; a root configured twice lists each file once.
    pub fn save_files(&self) -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        for root in &self.roots {
            for folder in self.scope.read_dir(root).unwrap_or_default() {
                if !self.scope.is_dir(&folder) {
                    continue;
                }
                let Some(fname) = folder.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                for f in self.scope.read_dir(&folder).unwrap_or_default() {
                    let Some(name) = f.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if BG3_SAVE.matches_file(name) && self.scope.is_file(&f) {
                        out.push((f.clone(), format!("{fname}/{name}")));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    /// The save whose `saveRef` is `wanted`.
    pub fn find(&self, wanted: &str) -> Option<(PathBuf, String)> {
        self.save_files()
            .into_iter()
            .find(|(abs, rel)| self.save_ref(abs, rel).as_deref() == Some(wanted))
    }

    /// Run `f` over one save's package; `None` when it is too large or not a
    /// readable LSPK.
    fn with_package<T>(
        &self,
        abs: &Path,
        f: impl FnOnce(&Package<'_>, std::fs::Metadata) -> T,
    ) -> Option<T> {
        let meta = self.scope.metadata(abs).ok()?;
        if meta.len() > MAX_LSV_BYTES {
            return None;
        }
        let bytes = self.scope.read(abs).ok()?;
        let package = read_lspk(&bytes).ok()?;
        Some(f(&package, meta))
    }

    /// Open one save for the listing: its row (which runs the NewAge check).
    pub fn open(&self, abs: &Path, rel: &str) -> Option<Opened> {
        self.with_package(abs, |package, meta| {
            let folder = rel.split('/').next().unwrap_or_default();
            let info = extract_save_info(package).ok();
            let party = party_counts(info.as_ref());
            let globals = read_limited(package, GLOBALS_LSF_NAME, GLOBALS_MAX_BYTES);
            let save_ref = self.save_ref(abs, rel);
            let row = self.row(
                package,
                save_ref,
                folder,
                info.as_ref(),
                party,
                meta.modified().ok(),
                globals.as_deref(),
            );
            Opened {
                row,
                globals,
                party_size: party.0.unwrap_or(0),
            }
        })
    }

    /// Open one save for the structure read: `Globals.lsf` and the party
    /// size only, with no listing row, so `NewAge` is decompressed ONCE (by
    /// the decoder's `read`) per request.
    pub fn open_structure(&self, abs: &Path) -> Option<Opened> {
        self.with_package(abs, |package, _| {
            let info = extract_save_info(package).ok();
            let party = party_counts(info.as_ref());
            let globals = read_limited(package, GLOBALS_LSF_NAME, GLOBALS_MAX_BYTES);
            Opened {
                row: Value::Null,
                globals,
                party_size: party.0.unwrap_or(0),
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        package: &Package<'_>,
        save_ref: Option<String>,
        folder: &str,
        info: Option<&Value>,
        (party, non_summon): (Option<usize>, Option<usize>),
        mtime: Option<std::time::SystemTime>,
        globals: Option<&[u8]>,
    ) -> Value {
        let (key, source) = campaign_key_from_package(package, folder);
        let meta = read_limited(package, META_LSF_NAME, META_LSF_MAX_BYTES);
        // A version is reported only for a file that parses (Codex P2): the
        // small meta.lsf is read in full; Globals.lsf has its tables parsed
        // and its values section bounds-checked (the decoder reads the blob).
        let meta_ok = meta
            .as_deref()
            .filter(|m| read_lsf_attribute(m, "\0").is_ok());
        let meta_version = meta_ok
            .and_then(lsf_parsed_version)
            .filter(|v| (1..=255).contains(v));
        let newage = globals.map(|g| lsf_attribute_len(g, NEWAGE_ATTRIBUTE));
        let readable = globals.map_or(Readability::NotChecked, |g| self.decoder.readable(g));
        // A Globals values section that fails to decode reports no version.
        let globals_version = globals
            .and_then(lsf_parsed_version)
            .filter(|v| (1..=255).contains(v))
            .filter(|_| readable != Readability::LsfUnreadable);
        let fallback = fallback_reason(
            package.find(META_LSF_NAME).is_some(),
            meta_version,
            globals_version,
            &newage,
            readable,
        );
        let version = info
            .and_then(|i| i.get("Game Version"))
            .and_then(Value::as_str)
            .and_then(game_version);
        json!({
            "saveRef": save_ref,
            "campaignKeyPrefix": key.get(..8).unwrap_or("00000000"),
            "campaignKeySource": serde_json::to_value(source).unwrap_or(Value::Null),
            "gameVersion": version,
            "savedAt": mtime.map(|m| captured_at(Some(m))),
            "partySize": party,
            "nonSummonCount": non_summon,
            // meta.lsf `ClientDatas/ClientData` needs a node walk the reader
            // does not do yet (S2 tree walker); `null` = not read.
            "clientCount": Value::Null,
            "lsfVersions": { "meta": meta_version, "globals": globals_version },
            "newAgeBytes": newage.and_then(Result::ok).flatten().unwrap_or(0).min(1 << 30),
            "fallbackReason": fallback,
        })
    }
}

fn fallback_reason(
    has_meta: bool,
    meta_version: Option<u32>,
    globals_version: Option<u32>,
    newage: &Option<Result<Option<u64>, rl_collectors::LsfError>>,
    readable: Readability,
) -> Option<&'static str> {
    if !has_meta {
        return Some("meta-missing");
    }
    match newage {
        _ if meta_version.is_none() || globals_version.is_none() => Some("lsf-unreadable"),
        None | Some(Err(_)) => Some("lsf-unreadable"),
        Some(Ok(None | Some(0))) => Some("no-newage"),
        Some(Ok(Some(_))) if readable == Readability::LsfUnreadable => Some("lsf-unreadable"),
        Some(Ok(Some(_))) if readable == Readability::NotLsmf => Some("newage-unreadable"),
        Some(Ok(Some(_))) => None,
    }
}

fn read_limited(package: &Package<'_>, name: &str, limit: u64) -> Option<Vec<u8>> {
    let entry = package.find(name)?;
    package.read_entry_limited(entry, limit).ok()
}

/// `(partySize, nonSummonCount)` from `SaveInfo.json`; `None` past 16.
fn party_counts(info: Option<&Value>) -> (Option<usize>, Option<usize>) {
    let chars = info
        .and_then(|i| i.get("Active Party"))
        .and_then(|p| p.get("Characters"))
        .and_then(Value::as_array);
    let Some(chars) = chars else {
        return (None, None);
    };
    let non_summon = chars.iter().filter(|c| c.get("Classes").is_some()).count();
    let cap = |n: usize| (n <= 16).then_some(n);
    (cap(chars.len()), cap(non_summon))
}
