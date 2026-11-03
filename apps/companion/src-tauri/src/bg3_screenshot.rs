//! The BG3 screenshot collector (ROK-1966, contract `bg3-screenshot/v1`):
//! finds new screenshots in Steam's per-game folder (app 1086940), re-encodes
//! each to a WebP ≤ 1 MiB ([`crate::screenshot_encode`]) and emits one message
//! per screenshot for the hub the user picked. Off by default; only files
//! taken at or after the opt-in time are sent (no backfill). The dedupe item is
//! the sha256 of the ORIGINAL file, so a re-encode never re-sends.

use crate::collector::{
    captured_at, envelope, scope_key, Blob, Collector, PendingUpload, Scan, SentState,
};
use crate::manifest::BG3_SCREENSHOT;
use crate::scope::WatchScope;
use crate::screenshot_encode::encode_screenshot;
use rl_collectors::sha256_hex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONTRACT: &str = "bg3-screenshot";
pub const CONTRACT_VERSION: u32 = 1;
/// The blob slot the contract registers (`blobSlots: ['image']`).
pub const BLOB_SLOT: &str = "image";
/// Under one `userdata/<steamid>`: Steam's screenshots for BG3.
pub const STEAM_SUBPATH: [&str; 4] = ["760", "remote", "1086940", "screenshots"];
/// Larger originals are skipped (a 16K PNG is well under this).
pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// New screenshots waiting for a hub pick, from the app's last scan.
pub static WAITING: AtomicUsize = AtomicUsize::new(0);
/// Originals that could not be re-encoded: refused for this run, logged once.
static FAILED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

pub struct Bg3ScreenshotCollector {
    roots: Vec<PathBuf>,
    scope: WatchScope,
    opted_in_at: Option<i64>,
    hub: Option<i64>,
    waiting: AtomicUsize,
    report: Option<&'static AtomicUsize>,
    encodes: AtomicUsize,
}

impl Bg3ScreenshotCollector {
    /// `roots` are Steam `userdata` folders (or a screenshots folder the
    /// user picked); reads are confined to `scope`.
    pub fn scoped(roots: &[PathBuf], scope: WatchScope) -> Self {
        Self {
            roots: roots.to_vec(),
            scope,
            opted_in_at: None,
            hub: None,
            waiting: AtomicUsize::new(0),
            report: None,
            encodes: AtomicUsize::new(0),
        }
    }

    /// Only files whose mtime is at or after `at` (unix seconds) are sent.
    /// `None` sends nothing.
    pub fn with_opt_in(mut self, at: Option<i64>) -> Self {
        self.opted_in_at = at;
        self
    }

    /// The hub screenshots go to; `None` holds them back.
    pub fn with_hub(mut self, hub: Option<i64>) -> Self {
        self.hub = hub;
        self
    }

    /// Each scan also stores its waiting count in `slot` (the app's [`WAITING`]).
    pub fn reporting(mut self, slot: &'static AtomicUsize) -> Self {
        self.report = Some(slot);
        self
    }

    /// New screenshots the last scan held back for want of a hub.
    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Relaxed)
    }

    /// How many screenshots this collector re-encoded.
    pub fn encodes(&self) -> usize {
        self.encodes.load(Ordering::Relaxed)
    }

    /// Screenshot folders: `<root>/<steamid>/760/remote/1086940/screenshots`
    /// for every Steam user under a root; a root with none is itself the
    /// folder (a manual override).
    pub fn folders(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for root in self.roots.iter().filter(|r| self.scope.is_dir(r)) {
            let found: Vec<PathBuf> = self
                .scope
                .read_dir(root)
                .unwrap_or_default()
                .into_iter()
                .map(|user| STEAM_SUBPATH.iter().fold(user, |p, s| p.join(s)))
                .filter(|p| self.scope.is_dir(p))
                .collect();
            if found.is_empty() {
                out.push(root.clone());
            } else {
                out.extend(found);
            }
        }
        out
    }

    /// Every top-level file in [`Self::folders`] the manifest names, newest
    /// first (Steam's `thumbnails/` sub-folder is never read).
    pub fn files(&self) -> Vec<PathBuf> {
        let mut stamped: Vec<(PathBuf, Option<SystemTime>)> = self
            .folders()
            .iter()
            .flat_map(|f| self.scope.read_dir(f).unwrap_or_default())
            .filter(|f| is_named(f) && self.scope.is_file(f))
            .map(|f| {
                let at = self.scope.metadata(&f).and_then(|m| m.modified()).ok();
                (f, at)
            })
            .collect();
        stamped.sort_by(|(pa, a), (pb, b)| b.cmp(a).then_with(|| pa.cmp(pb)));
        stamped.into_iter().map(|(p, _)| p).collect()
    }

    /// The original's bytes and mtime when it was taken at/after `cutoff`.
    fn read_new(&self, path: &Path, cutoff: i64, scan: &mut Scan) -> Option<(Vec<u8>, SystemTime)> {
        let name = path.file_name()?.to_string_lossy().into_owned();
        let meta = self.scope.metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        if unix(mtime) < cutoff {
            return None;
        }
        if meta.len() > MAX_SOURCE_BYTES {
            scan.notes
                .push(format!("{name}: screenshot is over 64 MiB; skipped"));
            return None;
        }
        match self.scope.read(path) {
            Ok(bytes) => {
                scan.files += 1;
                Some((bytes, mtime))
            }
            Err(e) => {
                scan.notes
                    .push(format!("{name}: cannot read screenshot ({e})"));
                None
            }
        }
    }

    fn upload(
        &self,
        name: &str,
        bytes: &[u8],
        sha: String,
        at: SystemTime,
        hub: i64,
        origin: &str,
    ) -> Result<PendingUpload, String> {
        self.encodes.fetch_add(1, Ordering::Relaxed);
        let (body, blob) = build(bytes, Some(at), hub)?;
        Ok(PendingUpload {
            contract: CONTRACT,
            scope: scope_key(origin, BG3_SCREENSHOT.id),
            item: sha.clone(),
            sha256: sha,
            label: format!("Screenshot {name}"),
            body,
            blobs: vec![blob],
            campaign: None,
        })
    }
}

fn is_named(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| BG3_SCREENSHOT.matches_file(n))
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// One screenshot's message and its WebP blob, for hub `hub`.
pub fn build(bytes: &[u8], mtime: Option<SystemTime>, hub: i64) -> Result<(Value, Blob), String> {
    let enc = encode_screenshot(bytes)?;
    let webp_sha = sha256_hex(&enc.webp);
    // takenAt first: it can never be after capturedAt (contract §4.3).
    let taken_at = captured_at(mtime);
    let payload = json!({
        "takenAt": taken_at,
        "width": enc.source_width,
        "height": enc.source_height,
        "image": {
            "sha256": webp_sha,
            "byteLength": enc.webp.len(),
            "contentType": "image/webp",
            "width": enc.width,
            "height": enc.height,
        },
        "target": { "hubGuildId": hub },
    });
    let body = envelope(CONTRACT, CONTRACT_VERSION, &captured_at(None), payload);
    let blob = Blob {
        sha256: webp_sha,
        content_type: "image/webp".into(),
        bytes: enc.webp,
    };
    Ok((body, blob))
}

impl Collector for Bg3ScreenshotCollector {
    fn id(&self) -> &'static str {
        BG3_SCREENSHOT.id
    }

    /// The screenshot folders themselves, not all of `userdata`.
    fn watch_roots(&self) -> Vec<PathBuf> {
        self.folders()
    }

    fn scan(&self, origin: &str, sent: &SentState) -> Scan {
        let mut scan = Scan::default();
        let Some(cutoff) = self.opted_in_at else {
            scan.notes
                .push("BG3 screenshots: no opt-in time; turn uploads off and on".into());
            return scan;
        };
        let done = sent.scope(&scope_key(origin, BG3_SCREENSHOT.id));
        let mut waiting = 0;
        for path in self.files() {
            let Some((bytes, at)) = self.read_new(&path, cutoff, &mut scan) else {
                continue;
            };
            let sha = sha256_hex(&bytes);
            if done.contains_key(&sha) || refused(&sha) {
                continue;
            }
            let Some(hub) = self.hub else {
                waiting += 1;
                continue;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match self.upload(&name, &bytes, sha.clone(), at, hub, origin) {
                Ok(p) => scan.pending.push(p),
                Err(e) => {
                    refuse(&sha);
                    scan.notes.push(format!("{name}: not uploaded, {e}"));
                }
            }
        }
        if waiting > 0 {
            scan.notes.push(format!(
                "BG3 screenshots: {waiting} waiting; pick a hub to send them"
            ));
        }
        self.waiting.store(waiting, Ordering::Relaxed);
        if let Some(slot) = self.report {
            slot.store(waiting, Ordering::Relaxed);
        }
        scan
    }
}

fn refused(sha: &str) -> bool {
    FAILED.lock().map(|s| s.contains(sha)).unwrap_or(false)
}

fn refuse(sha: &str) {
    if let Ok(mut s) = FAILED.lock() {
        s.insert(sha.to_owned());
    }
}
