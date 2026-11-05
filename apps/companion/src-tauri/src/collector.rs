//! Pieces every game collector shares (ROK-1961 lane C-3): the [`Collector`]
//! trait, one pending upload, the envelope, and the persisted dedupe state.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// File in the app data dir holding [`SentState`].
pub const SENT_STATE_FILE: &str = "sent-state.json";

/// The envelope's `appVersion` (`MAJOR.MINOR.PATCH`).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Companion envelope version (envelope/v1).
pub const ENVELOPE_VERSION: u32 = 1;

/// One message ready to send, plus what to record once the server settled it.
#[derive(Debug, Clone)]
pub struct PendingUpload {
    /// Contract name — the last segment of the ingest URL.
    pub contract: &'static str,
    /// Dedupe scope ([`scope_key`]: server origin + one SavedVariables file)
    /// and the item inside it (a GUID).
    pub scope: String,
    pub item: String,
    pub sha256: String,
    /// Short label for the window. Local only, never sent.
    pub label: String,
    /// The full message (envelope + payload).
    pub body: Value,
    /// Blobs the message references (e.g. a BG3 thumbnail), PUT before it.
    pub blobs: Vec<Blob>,
    /// The BG3 campaign the save belongs to, so a batch can skip a campaign
    /// the user unticks mid-batch (ROK-2068). `None` for other games.
    pub campaign: Option<String>,
}

/// One content-addressed blob (`PUT /companion/v1/blobs/:sha256`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    pub sha256: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// What one scan found.
#[derive(Debug, Default)]
pub struct Scan {
    /// Messages whose content differs from the last settled upload.
    pub pending: Vec<PendingUpload>,
    /// Local-only notes (skipped entries, unreadable files) for the log.
    pub notes: Vec<String>,
    /// How many source files were read.
    pub files: usize,
    /// Items found but not queued yet (BG3 screenshots waiting for a hub
    /// pick): still unsettled, so they count in `/dev/v1/collectors` pending.
    pub waiting: usize,
}

/// A game collector: where to watch, which changes matter, and how to turn
/// the files into messages.
pub trait Collector: Send + Sync {
    /// Stable id, e.g. `"wow-ledgerlink"`.
    fn id(&self) -> &'static str;
    /// Folders to watch recursively. May be empty (nothing installed yet).
    fn watch_roots(&self) -> Vec<PathBuf>;
    // Which changed files matter is the manifest's `file_patterns`
    // (`registry::Entry::wants`), not a per-collector method (ROK-2061).
    /// Read the files and return every message that is new versus what
    /// `origin` (the configured server) already settled in `sent`.
    fn scan(&self, origin: &str, sent: &SentState) -> Scan;
}

/// The dedupe scope for one source file on one server: `{origin}|{path}`.
/// Keyed by origin so an export a test server settled is still sent to prod,
/// and switching back to a server keeps its history.
pub fn scope_key(origin: &str, path: &str) -> String {
    format!("{origin}|{path}")
}

/// `scope → item → sha256` of the last upload the server settled (accepted,
/// or rejected terminally). Survives restarts, so an unchanged export is never
/// re-sent and the user's import budget is kept.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentState(pub BTreeMap<String, BTreeMap<String, String>>);

impl SentState {
    /// Missing or unreadable file → empty state (first run).
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

    /// The `item → sha256` map for one scope (empty if never seen).
    pub fn scope(&self, scope: &str) -> BTreeMap<String, String> {
        self.0.get(scope).cloned().unwrap_or_default()
    }

    pub fn record(&mut self, scope: &str, item: &str, sha256: &str) {
        self.0
            .entry(scope.to_owned())
            .or_default()
            .insert(item.to_owned(), sha256.to_owned());
    }
}

/// Wrap a payload in the companion envelope (envelope/v1 §2).
pub fn envelope(contract: &str, contract_version: u32, captured_at: &str, payload: Value) -> Value {
    json!({
        "envelopeVersion": ENVELOPE_VERSION,
        "contract": contract,
        "contractVersion": contract_version,
        "capturedAt": captured_at,
        "appVersion": APP_VERSION,
        "collectorVersion": rl_collectors::COLLECTOR_VERSION,
        "payload": payload,
    })
}

/// `capturedAt` / `savedAt` from a file mtime, never in the future (the
/// server rejects more than 5 minutes ahead of its clock).
pub fn captured_at(mtime: Option<std::time::SystemTime>) -> String {
    let now = std::time::SystemTime::now();
    let t = mtime.filter(|m| *m <= now).unwrap_or(now);
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rl_collectors::rfc3339_utc_from_unix_seconds(i64::try_from(secs).unwrap_or(0))
}
