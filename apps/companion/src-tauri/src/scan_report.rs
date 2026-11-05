//! What `/dev/v1/collectors` reports beyond the window's rows: when each
//! collector last scanned, how many items that scan left unsettled, and when
//! the server last accepted an upload from it. The accepted-upload time is
//! persisted next to `sent-state.json`, so a restart does not blank it.
//!
//! Also the bridge-safe form of an Activity line ([`bridge_line`]), mirrored
//! into the dev-bridge log ring.

use crate::collector::{PendingUpload, SentState};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// File next to `sent-state.json`: `collector id → last accepted upload`.
pub const UPLOAD_TIMES_FILE: &str = "upload-times.json";

/// One collector's line in `/dev/v1/collectors` (beyond `enabled`/`paused`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub last_scan_at: Option<String>,
    pub pending: usize,
    pub last_upload_at: Option<String>,
}

/// The persisted accepted-upload times (RFC 3339 UTC per collector id).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadTimes(pub BTreeMap<String, String>);

impl UploadTimes {
    /// Missing or unreadable file (an install that predates it) → empty.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write via a temp file + rename, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

/// Where [`UploadTimes`] lives: beside the sent-state file.
pub fn upload_times_path(sent_path: &Path) -> PathBuf {
    sent_path.with_file_name(UPLOAD_TIMES_FILE)
}

/// Per-collector scan facts plus the persisted upload times.
#[derive(Debug, Default, Clone)]
pub struct ScanBook {
    scans: BTreeMap<String, (String, usize)>,
    pub uploads: UploadTimes,
}

impl ScanBook {
    pub fn new(uploads: UploadTimes) -> Self {
        Self {
            scans: BTreeMap::new(),
            uploads,
        }
    }

    /// A scan of `id` ran at `at` and left `pending` items unsettled.
    pub fn scanned(&mut self, id: &str, at: String, pending: usize) {
        self.scans.insert(id.to_owned(), (at, pending));
    }

    /// `id` is switched off: it did not scan, so it reports no scan.
    pub fn forget_scan(&mut self, id: &str) {
        self.scans.remove(id);
    }

    /// The server accepted an upload from `id` at `at` (kept if later).
    pub fn uploaded(&mut self, id: &str, at: String) {
        let slot = self.uploads.0.entry(id.to_owned()).or_default();
        if at > *slot {
            *slot = at;
        }
    }

    pub fn reports(&self) -> BTreeMap<String, ScanReport> {
        let ids = self.scans.keys().chain(self.uploads.0.keys());
        ids.map(|id| {
            let scan = self.scans.get(id);
            let report = ScanReport {
                last_scan_at: scan.map(|(at, _)| at.clone()),
                pending: scan.map_or(0, |(_, n)| *n),
                last_upload_at: self.uploads.0.get(id).cloned(),
            };
            (id.clone(), report)
        })
        .collect()
    }
}

/// How many of a scan's items are still unsettled in `sent` (neither
/// accepted nor rejected terminally): what is queued for a later pass.
pub fn unsettled(sent: &SentState, pending: &[PendingUpload]) -> usize {
    pending
        .iter()
        .filter(|p| sent.scope(&p.scope).get(&p.item) != Some(&p.sha256))
        .count()
}

fn abs_path() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    // A drive-letter or UNC path, or a rooted Unix path of 2+ segments, that
    // starts a word. It runs to the last non-space before `(`, a quote or the
    // line end, so a folder with spaces (`World of Warcraft`) goes too.
    P.get_or_init(|| {
        let tail = r#"(?:[^()"'\r\n]*[^\s()"'])?"#;
        let head = r#"(^|[\s(])((?:[A-Za-z]:[\\/]|\\\\|/[^\s()"'/]+/)"#;
        Regex::new(&format!("{head}{tail})")).unwrap_or_else(|_| unreachable!("static pattern"))
    })
}

/// An Activity line as the dev bridge may store it. A collector line's
/// `<subject>: ` prefix is a character, account, save or campaign name
/// (`Thrall: Uploaded`), so it is dropped; any absolute path becomes
/// `<path>`. The ring then applies its own redaction (tokens, GUIDs, homes).
pub fn bridge_line(collector_id: Option<&str>, text: &str) -> String {
    let body = match (collector_id, text.split_once(": ")) {
        (Some(_), Some((_, rest))) => rest,
        _ => text,
    };
    abs_path().replace_all(body, "$1<path>").into_owned()
}

/// The ring's `target` for an Activity line.
pub fn bridge_target(collector_id: Option<&str>) -> String {
    match collector_id {
        Some(id) => format!("collector:{id}"),
        None => "collector".to_owned(),
    }
}
