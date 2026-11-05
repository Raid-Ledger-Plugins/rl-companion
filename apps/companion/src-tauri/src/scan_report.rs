//! What `/dev/v1/collectors` reports beyond the window's rows: when each
//! collector last scanned, how many items that scan left unsettled, and when
//! the server last accepted an upload from it. The accepted-upload time is
//! persisted next to `sent-state.json`, so a restart does not blank it.
//!
//! Also the bridge-safe form of an Activity line ([`scrub_paths`], [`note_body`]), mirrored
//! into the dev-bridge log ring: no subject (a character, account, save or
//! campaign name) and no absolute path.

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

/// How many of a scan's items are still unsettled in `sent` (neither
/// accepted nor rejected terminally): what is queued for a later pass.
pub fn unsettled(sent: &SentState, pending: &[PendingUpload]) -> usize {
    pending
        .iter()
        .filter(|p| sent.0.get(&p.scope).and_then(|m| m.get(&p.item)) != Some(&p.sha256))
        .count()
}

fn abs_path() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    // Where an absolute path starts: a drive (`C:\`, `C:/`), UNC or verbatim
    // (`\\host\share`, `\\?\C:\`), home (`~/`), or a rooted Unix path.
    const HEAD: &str = r#"(?:[A-Za-z]:[\\/]|\\\\|~[\\/]|/[^\s\\/"'()\[\]{},;])"#;
    // Quoted (`"D:\Program Files (x86)\x"`, notify's `about ["…"]`, Rust's
    // `{:?}`): everything up to the closing quote is the path.
    const QUOTED: &str = r#"(?P<dq>")HEAD[^"\r\n]*|(?P<sq>')HEAD[^'\r\n]*"#;
    // Bare: after a word break, every segment that a separator follows may
    // hold spaces and parentheses (`World of Warcraft\`, `Program Files
    // (x86)\`); the last one runs to the next space, so ` (denied)` stays.
    const BARE: &str = r#"(?P<b>^|[\s(\[{=,;])HEAD(?:[^\\/"'\r\n]*[\\/])*[^\s\\/"'()\[\]{},;]*"#;
    P.get_or_init(|| {
        let pattern = format!("{QUOTED}|{BARE}").replace("HEAD", HEAD);
        Regex::new(&pattern).unwrap_or_else(|_| unreachable!("static pattern"))
    })
}

/// `text` with every absolute path replaced by `<path>`, to the end of the
/// path (a quoted path to its closing quote): an Activity line as the dev
/// bridge may store it, once the caller has left its subject out. The ring
/// then applies its own redaction (tokens, GUIDs, homes).
pub fn scrub_paths(text: &str) -> String {
    abs_path()
        .replace_all(text, |c: &regex::Captures<'_>| {
            let lead = ["dq", "sq", "b"]
                .iter()
                .find_map(|g| c.name(g))
                .map_or("", |m| m.as_str());
            format!("{lead}<path>")
        })
        .into_owned()
}

/// A collector scan note's text without its `<account or file>: ` prefix.
/// Notes are built as `"{subject}: {text}"` from a WoW account folder or a
/// save file's name; neither can hold `": "` on Windows, so the first one ends
/// the subject. (Names typed by the player — save or campaign names — never
/// go through here: their callers pass the subject apart, see
/// `Runtime::log_about`.)
pub fn note_body(note: &str) -> &str {
    note.split_once(": ").map_or(note, |(_, rest)| rest)
}

/// The ring's `target` for an Activity line.
pub fn bridge_target(collector_id: Option<&str>) -> String {
    match collector_id {
        Some(id) => format!("collector:{id}"),
        None => "collector".to_owned(),
    }
}
