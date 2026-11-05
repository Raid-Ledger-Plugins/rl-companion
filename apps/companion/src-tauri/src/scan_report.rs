//! What `/dev/v1/collectors` reports beyond the window's rows: when each
//! collector last scanned, how many items that scan left unsettled, and when
//! the server last accepted an upload from it. The accepted-upload time is
//! persisted next to `sent-state.json`, so a restart does not blank it.
//! Only ids, times and counts: no Activity text reaches the dev bridge.

use crate::collector::{PendingUpload, Scan, SentState};
use crate::registry::Entry;
use crate::runtime::Runtime;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, json.as_bytes())?;
        file.sync_all()?;
        drop(file);
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

/// One enabled collector's pass over a fresh scan. Dropping it stamps the
/// scan ([`Runtime::scanned`]) with what the pass left unsettled, so no return
/// path of the upload can skip the stamp.
pub struct ScanPass<'a> {
    rt: &'a Runtime,
    id: &'static str,
    pub scan: Scan,
}

impl<'a> ScanPass<'a> {
    /// Read `entry`; `None` (and its last scan forgotten) when it is off.
    pub fn begin(rt: &'a Runtime, entry: &Entry, origin: &str) -> Option<Self> {
        let Some(scan) = entry.scan(origin, &rt.sent()) else {
            rt.forget_scan(entry.manifest.id);
            return None;
        };
        Some(Self {
            rt,
            id: entry.manifest.id,
            scan,
        })
    }
}

impl Drop for ScanPass<'_> {
    fn drop(&mut self) {
        self.rt
            .scanned(self.id, &self.scan.pending, self.scan.waiting);
    }
}

/// How many of a scan's items are still unsettled in `sent` (neither
/// accepted nor rejected terminally): what is queued for a later pass.
pub fn unsettled(sent: &SentState, pending: &[PendingUpload]) -> usize {
    pending
        .iter()
        .filter(|p| sent.0.get(&p.scope).and_then(|m| m.get(&p.item)) != Some(&p.sha256))
        .count()
}
