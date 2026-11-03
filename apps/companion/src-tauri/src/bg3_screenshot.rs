//! The BG3 screenshot collector (ROK-1966, contract `bg3-screenshot/v1`):
//! finds new screenshots in Steam's per-game folder (app 1086940) and emits one
//! message per screenshot for the hub the user picked. Off by default; only
//! files taken at or after the opt-in time are sent (no backfill). The dedupe
//! item is the sha256 of the ORIGINAL file, so a re-encode never re-sends.
//!
//! A scan only identifies files: a known `(path, len, mtime)` is never re-read
//! ([`MEMO`]), and nothing is re-encoded. The WebP ≤ 1 MiB
//! ([`crate::screenshot_encode`]) is made by [`encode_for_send`], off the async
//! runtime, only once the upload is actually about to be sent.

use crate::collector::{
    captured_at, envelope, scope_key, Blob, Collector, PendingUpload, Scan, SentState,
};
use crate::manifest::BG3_SCREENSHOT;
use crate::scope::WatchScope;
use crate::screenshot_encode::encode_screenshot;
use crate::screenshot_preview::{self, Wanted};
use rl_collectors::sha256_hex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
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
/// `(canonical path, len, mtime)` -> sha256 of every original already read,
/// so an unchanged file is never re-read or re-hashed.
static MEMO: Mutex<BTreeMap<FileKey, String>> = Mutex::new(BTreeMap::new());
/// sha256 -> where the unsent original lives, for [`encode_for_send`].
static SOURCES: Mutex<BTreeMap<String, Source>> = Mutex::new(BTreeMap::new());
/// Past this many entries [`MEMO`] / [`SOURCES`] start over (a re-read, not a loss).
const MEMO_MAX: usize = 4096;

type FileKey = (PathBuf, u64, SystemTime);

/// An identified original: its hash, mtime and canonical path.
#[derive(Debug, Clone)]
struct Source {
    sha: String,
    mtime: SystemTime,
    path: PathBuf,
    scope: WatchScope,
}

pub struct Bg3ScreenshotCollector {
    roots: Vec<PathBuf>,
    scope: WatchScope,
    opted_in_at: Option<i64>,
    hub: Option<i64>,
    waiting: AtomicUsize,
    report: Option<&'static AtomicUsize>,
    reads: AtomicUsize,
    default_roots: bool,
    previewing: bool,
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
            reads: AtomicUsize::new(0),
            default_roots: false,
            previewing: false,
        }
    }

    /// `true` when `roots` are the default Steam `userdata` folders (no user
    /// override): a root without a BG3 screenshots folder is then skipped
    /// instead of being read as the folder itself.
    pub fn with_default_roots(mut self, default_roots: bool) -> Self {
        self.default_roots = default_roots;
        self
    }

    /// `true` while the first-upload preview is pending: the newest pending
    /// upload then carries its original as a preview-only blob.
    pub fn previewing(mut self, on: bool) -> Self {
        self.previewing = on;
        self
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

    /// How many originals this collector read from disk.
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    /// Screenshot folders: `<root>/<steamid>/760/remote/1086940/screenshots`
    /// for every Steam user under a root; an overridden root with none is
    /// itself the folder, a default root with none has no folder.
    pub fn folders(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for root in self.roots.iter().filter(|r| self.scope.is_dir(r)) {
            let candidates: Vec<PathBuf> = self
                .scope
                .read_dir(root)
                .unwrap_or_default()
                .into_iter()
                .map(|user| STEAM_SUBPATH.iter().fold(user, |p, s| p.join(s)))
                .filter(|p| self.scope.is_dir(p))
                .collect();
            if candidates.is_empty() {
                if !self.default_roots {
                    out.push(root.clone());
                }
            } else {
                out.extend(candidates.into_iter().filter(|p| is_bg3_folder(root, p)));
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

    /// The original's hash and mtime when it was taken at/after `cutoff`,
    /// read through `scope` (the screenshot folders only) unless [`MEMO`]
    /// already knows this `(path, len, mtime)`.
    fn identify(
        &self,
        path: &Path,
        cutoff: i64,
        scope: &WatchScope,
        scan: &mut Scan,
    ) -> Option<Source> {
        let name = path.file_name()?.to_string_lossy().into_owned();
        let real = scope.check(path).ok()?;
        let meta = scope.metadata(&real).ok()?;
        let mtime = meta.modified().ok()?;
        if unix(mtime) < cutoff {
            return None;
        }
        if meta.len() > MAX_SOURCE_BYTES {
            scan.notes
                .push(format!("{name}: screenshot is over 64 MiB; skipped"));
            return None;
        }
        scan.files += 1;
        let key = (real.clone(), meta.len(), mtime);
        let known = MEMO.lock().ok().and_then(|m| m.get(&key).cloned());
        let sha = match known {
            Some(sha) => sha,
            None => match scope.read(&real) {
                Ok(bytes) => {
                    self.reads.fetch_add(1, Ordering::Relaxed);
                    let sha = sha256_hex(&bytes);
                    remember(&MEMO, key, sha.clone());
                    sha
                }
                Err(e) => {
                    scan.notes
                        .push(format!("{name}: cannot read screenshot ({e})"));
                    return None;
                }
            },
        };
        Some(Source {
            sha,
            mtime,
            path: real,
            scope: scope.clone(),
        })
    }

    /// A message waiting for its WebP: `takenAt` and the hub are known, the
    /// image is filled in by [`encode_for_send`].
    fn unencoded(&self, name: &str, src: &Source, hub: i64, origin: &str) -> PendingUpload {
        let payload = json!({
            "takenAt": captured_at(Some(src.mtime)),
            "target": { "hubGuildId": hub },
        });
        PendingUpload {
            contract: CONTRACT,
            scope: scope_key(origin, BG3_SCREENSHOT.id),
            item: src.sha.clone(),
            sha256: src.sha.clone(),
            label: format!("Screenshot {name}"),
            body: envelope(CONTRACT, CONTRACT_VERSION, &captured_at(None), payload),
            blobs: Vec::new(),
            campaign: None,
        }
    }

    /// The preview image for the newest pending screenshot: Steam's own
    /// thumbnail or a cached one now, else a downscale queued for
    /// [`screenshot_preview::attach`]. Never the original.
    fn preview_blob(&self, src: &Source) -> Option<Blob> {
        let blob = screenshot_preview::ready(&src.sha, &src.path, &src.scope);
        if blob.is_none() {
            screenshot_preview::want(Wanted {
                sha: src.sha.clone(),
                path: src.path.clone(),
                scope: src.scope.clone(),
            });
        }
        blob
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
        let read_scope = WatchScope::new(&self.folders());
        let mut waiting = 0;
        for path in self.files() {
            let Some(src) = self.identify(&path, cutoff, &read_scope, &mut scan) else {
                continue;
            };
            if done.contains_key(&src.sha) || refused(&src.sha) {
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
            let mut p = self.unencoded(&name, &src, hub, origin);
            // `files()` is newest first: the first pending is the preview's.
            if self.previewing && scan.pending.is_empty() {
                p.blobs.extend(self.preview_blob(&src));
            }
            remember(&SOURCES, src.sha.clone(), src);
            scan.pending.push(p);
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

fn remember<K: Ord, V>(map: &Mutex<BTreeMap<K, V>>, key: K, value: V) {
    if let Ok(mut m) = map.lock() {
        if m.len() >= MEMO_MAX {
            m.clear();
        }
        m.insert(key, value);
    }
}

/// Zero the picker's waiting count (the collector was switched off).
pub fn clear_waiting() {
    WAITING.store(0, Ordering::Relaxed);
}

/// `true` for a screenshot upload whose WebP is not made yet.
pub fn needs_encode(p: &PendingUpload) -> bool {
    p.contract == CONTRACT && p.body.pointer("/payload/image").is_none()
}

/// Why a send-time encode did not produce a message.
#[derive(Debug)]
pub enum Unready {
    /// Try again on a later scan (original moved or changed).
    Retry(String),
    /// This original is refused for the rest of the run.
    Refuse(String),
}

/// Re-read `p`'s original, check it is still the scanned file, re-encode it
/// and return the complete message. Blocking: run it via [`encode_for_send`].
pub fn finish(p: &PendingUpload) -> Result<PendingUpload, Unready> {
    let src = SOURCES
        .lock()
        .ok()
        .and_then(|m| m.get(&p.sha256).cloned())
        .ok_or_else(|| Unready::Retry("original not found; retrying later".into()))?;
    let hub = p
        .body
        .pointer("/payload/target/hubGuildId")
        .and_then(Value::as_i64)
        .ok_or_else(|| Unready::Refuse("no hub in the message".into()))?;
    let bytes = read_capped(&src.scope, &src.path, MAX_SOURCE_BYTES)?;
    if sha256_hex(&bytes) != p.sha256 {
        return Err(Unready::Retry("screenshot changed; retrying later".into()));
    }
    let (body, blob) = build(&bytes, Some(src.mtime), hub).map_err(Unready::Refuse)?;
    Ok(PendingUpload {
        contract: p.contract,
        scope: p.scope.clone(),
        item: p.item.clone(),
        sha256: p.sha256.clone(),
        label: p.label.clone(),
        body,
        blobs: vec![blob],
        campaign: p.campaign.clone(),
    })
}

/// `folder` (`<root>/<steamid>/760/remote/1086940/screenshots`) is a BG3
/// folder only when it resolves to exactly that path under the resolved root:
/// a symlink or junction anywhere along it (e.g. `1086940/screenshots` ->
/// another game's `screenshots`) is refused, even inside the same `userdata`.
pub fn is_bg3_folder(root: &Path, folder: &Path) -> bool {
    let Some(user) = folder
        .ancestors()
        .nth(STEAM_SUBPATH.len())
        .and_then(Path::file_name)
    else {
        return false;
    };
    let (Ok(root), Ok(real)) = (std::fs::canonicalize(root), std::fs::canonicalize(folder)) else {
        return false;
    };
    let expected = STEAM_SUBPATH.iter().fold(root.join(user), |p, s| p.join(s));
    real == expected
}

/// Read `path` through `scope`, at most `cap` bytes: a file that grew past the
/// cap since the scan is never read whole into memory.
pub fn read_capped(scope: &WatchScope, path: &Path, cap: u64) -> Result<Vec<u8>, Unready> {
    use std::io::Read;
    let cannot = |e: std::io::Error| Unready::Retry(format!("cannot read screenshot ({e})"));
    let real = scope.check(path).map_err(|e| cannot(e.into()))?;
    let mut bytes = Vec::new();
    std::fs::File::open(real)
        .map_err(cannot)?
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(cannot)?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > cap) {
        return Err(Unready::Retry(
            "screenshot is now over 64 MiB; retrying later".into(),
        ));
    }
    Ok(bytes)
}

/// Whether screenshots may still be read and sent: the collector switch is
/// still on and has not been switched off and on again (a new opt-in time)
/// since `scanned` — the config the batch was scanned with.
pub fn still_opted_in(scanned: &crate::config::AppConfig, live: &crate::config::AppConfig) -> bool {
    let id = BG3_SCREENSHOT.id;
    live.collector_enabled(id, BG3_SCREENSHOT.default_enabled)
        && live.opted_in_at(id).is_some()
        && live.opted_in_at(id) == scanned.opted_in_at(id)
}

/// Run `job` (a decode + encode) on the blocking pool. An encode error or a
/// panic (`image`/libwebp parse untrusted files) refuses `sha` for the rest
/// of the run and is returned as a log line; the caller's loop carries on.
pub async fn off_thread<T, F>(sha: &str, job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Unready> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(job).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(Unready::Retry(e))) => Err(e),
        Ok(Err(Unready::Refuse(e))) => {
            refuse(sha);
            Err(format!("not uploaded, {e}"))
        }
        Err(e) => {
            refuse(sha);
            Err(format!("not uploaded, the image encoder crashed ({e})"))
        }
    }
}

/// [`finish`] off the async runtime, right before the upload is sent.
pub async fn encode_for_send(p: &PendingUpload) -> Result<PendingUpload, String> {
    // Everything but the preview blob, which may be a large original.
    let job = PendingUpload {
        contract: p.contract,
        scope: p.scope.clone(),
        item: p.item.clone(),
        sha256: p.sha256.clone(),
        label: p.label.clone(),
        body: p.body.clone(),
        blobs: Vec::new(),
        campaign: p.campaign.clone(),
    };
    off_thread(&p.sha256, move || finish(&job)).await
}

pub(crate) fn refused(sha: &str) -> bool {
    FAILED.lock().map(|s| s.contains(sha)).unwrap_or(false)
}

pub(crate) fn refuse(sha: &str) {
    if let Ok(mut s) = FAILED.lock() {
        s.insert(sha.to_owned());
    }
}
