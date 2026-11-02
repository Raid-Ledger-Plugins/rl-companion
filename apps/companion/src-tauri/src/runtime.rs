//! Collector runtime: watch → settle → scan → upload, plus the status the
//! window shows. One loop task per app; "Upload now" and setting changes
//! kick it through [`Runtime::kick`].

use crate::blob_send::send_with_blobs;
use crate::collector::{PendingUpload, SentState};
use crate::collector_view::{device_note, device_state, rows, CollectorRowDto, RETRYING};
use crate::config::AppConfig;
use crate::gate::{gate, Gate};
use crate::manifest::{Platform, WOW_LEDGERLINK};
use crate::preview::Held;
use crate::registry::{CollectorRegistry, Entry};
use crate::state::Shared;
use crate::tray;
use crate::upload::{UploadOutcome, Uploader};
use crate::watch::{start_watcher, SettleSet};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

/// Event the window listens to.
pub const STATUS_EVENT: &str = "collector-status";
/// Full rescan even without a file event (missed events, new folders).
const PERIODIC_SCAN_SECS: u32 = 300;
const LOG_LINES: usize = 50;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CharResult {
    pub label: String,
    pub result: String,
    pub ok: bool,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub at: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CollectorStatusDto {
    pub enabled: bool,
    pub wow_dir: String,
    pub status: String,
    pub results: Vec<CharResult>,
    pub log: Vec<LogLine>,
    /// Device-level state (paused, not linked, rate limited), shown once.
    pub device: Option<String>,
    /// The server refused the device token (401) on the last scan and the
    /// token was cleared. The window keys off this, never off `status` text.
    pub relink: bool,
    /// One row per registered collector.
    pub collectors: Vec<CollectorRowDto>,
}

/// What to do after one upload ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    /// Record the sha so this content is never resent.
    pub record: bool,
    /// Stop the batch (401, 429, or a 4xx that is not about the message).
    pub stop: bool,
    /// Clear the device token (401).
    pub relink: bool,
    /// Send nothing before this has passed (429 `Retry-After`).
    pub hold: Option<Duration>,
    pub ok: bool,
    pub text: String,
}

fn settle(record: bool, stop: bool, ok: bool, text: String) -> Effect {
    Effect {
        record,
        stop,
        relink: false,
        hold: None,
        ok,
        text,
    }
}

/// Only accepted uploads and the server's content rejections are recorded;
/// everything else is tried again on a later scan.
pub fn effect(outcome: &UploadOutcome) -> Effect {
    match outcome {
        UploadOutcome::Accepted => settle(true, false, true, "Uploaded".to_owned()),
        UploadOutcome::Rejected {
            status,
            code,
            issue,
        } => {
            let detail = issue
                .as_deref()
                .map(|i| format!(" ({i})"))
                .unwrap_or_default();
            let text = format!("Rejected {status}: {code}{detail}");
            settle(true, false, false, text)
        }
        UploadOutcome::Unauthorized => Effect {
            relink: true,
            ..settle(false, true, false, "Re-link this device".to_owned())
        },
        UploadOutcome::RateLimited { wait } => {
            let text = format!("Rate limited; retrying in {}s", wait.as_secs());
            Effect {
                hold: Some(*wait),
                ..settle(false, true, false, text)
            }
        }
        UploadOutcome::Deferred { reason } => {
            settle(false, true, false, format!("Will retry: {reason}"))
        }
        UploadOutcome::GaveUp { reason } => {
            settle(false, false, false, format!("Will retry: {reason}"))
        }
    }
}

struct Inner {
    status: String,
    /// Keyed by `(scope, item)`: every BG3 save shares the item `save`, so the
    /// scope (one per save file) keeps one save's result from overwriting another's.
    results: BTreeMap<(String, String), CharResult>,
    log: VecDeque<LogLine>,
    sent: SentState,
    /// No uploads before this (a 429's `Retry-After`).
    hold_until: Option<Instant>,
    /// Each collector's own line from the last scan.
    lines: BTreeMap<String, String>,
    /// The last scan's device inputs; the window's device line is derived
    /// from them and the live hold at render time (ROK-2068).
    paused: bool,
    linked: Result<bool, String>,
    /// The last scan hit a 401 and cleared the token.
    relink: bool,
    /// Batches waiting on the user's first-upload confirmation.
    held: BTreeMap<String, Held>,
}

pub struct Runtime {
    sent_path: PathBuf,
    inner: Mutex<Inner>,
    kick: Notify,
}

impl Runtime {
    pub fn new(sent_path: PathBuf) -> Self {
        let sent = SentState::load(&sent_path);
        Self {
            sent_path,
            inner: Mutex::new(Inner {
                status: "Starting…".into(),
                results: BTreeMap::new(),
                log: VecDeque::new(),
                sent,
                hold_until: None,
                lines: BTreeMap::new(),
                paused: false,
                linked: Ok(true),
                relink: false,
                held: BTreeMap::new(),
            }),
            kick: Notify::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Ask the loop for a rescan now.
    pub fn kick(&self) {
        self.kick.notify_one();
    }

    pub fn sent(&self) -> SentState {
        self.lock().sent.clone()
    }

    pub fn log(&self, text: impl Into<String>) {
        let mut inner = self.lock();
        inner.log.push_front(LogLine {
            at: now(),
            text: text.into(),
        });
        inner.log.truncate(LOG_LINES);
    }

    /// Send nothing until `wait` has passed (429 `Retry-After`).
    pub fn hold_for(&self, wait: Duration) {
        self.lock().hold_until = Some(Instant::now() + wait);
    }

    /// Whole seconds left on a 429 hold (at least 1), or `None` once it passed.
    pub fn held_secs(&self) -> Option<u64> {
        secs_left(self.lock().hold_until, Instant::now())
    }

    pub fn set_status(&self, text: impl Into<String>) {
        self.lock().status = text.into();
    }

    /// Settle one upload: remember the result, and the sha if `record`.
    pub fn finish(&self, pending: &PendingUpload, fx: &Effect) {
        let mut inner = self.lock();
        let at = now();
        inner.results.insert(
            (pending.scope.clone(), pending.item.clone()),
            CharResult {
                label: pending.label.clone(),
                result: fx.text.clone(),
                ok: fx.ok,
                at,
            },
        );
        if fx.record {
            inner
                .sent
                .record(&pending.scope, &pending.item, &pending.sha256);
            if let Err(e) = inner.sent.save(&self.sent_path) {
                inner.log.push_front(LogLine {
                    at: now(),
                    text: format!("Could not save upload state ({e})"),
                });
            }
        }
    }

    /// Remember the scan's per-collector lines and the 401 flag.
    pub fn set_lines(&self, lines: BTreeMap<String, String>, relink: bool) {
        let mut inner = self.lock();
        inner.lines = lines;
        inner.relink = relink;
    }

    /// Remember the scan's device inputs (paused, linked).
    pub fn set_device(&self, paused: bool, linked: Result<bool, String>) {
        let mut inner = self.lock();
        inner.paused = paused;
        inner.linked = linked;
    }

    /// A scan is starting: a 401 from an earlier scan must not ride along on
    /// this scan's mid-scan updates ("Uploading…"). Only the scan's own 401
    /// sets it again, through `set_lines`.
    pub fn begin_scan(&self) {
        self.lock().relink = false;
    }

    /// The device linked (or re-linked): the old 401 no longer applies, and a
    /// scan runs now so the waiting exports go up with the new token.
    pub fn linked(&self) {
        self.lock().relink = false;
        self.kick();
    }

    /// Hold `id`'s batch behind its first-upload preview (replaces any
    /// earlier hold); `None` clears it.
    pub fn set_held(&self, id: &str, held: Option<Held>) {
        let mut inner = self.lock();
        // A scan that began before Skip re-holds keys already recorded as
        // handled; such a hold is stale and is dropped (review MINOR 5).
        let fresh = held.filter(|h| {
            h.keys
                .iter()
                .any(|(scope, item, sha)| inner.sent.scope(scope).get(item) != Some(sha))
        });
        match fresh {
            Some(h) => inner.held.insert(id.to_owned(), h),
            None => inner.held.remove(id),
        };
    }

    /// Skip: record every held item as handled so it is never sent. Returns
    /// how many were skipped.
    pub fn skip_held(&self, id: &str) -> usize {
        let mut inner = self.lock();
        let Some(held) = inner.held.remove(id) else {
            return 0;
        };
        for (scope, item, sha) in &held.keys {
            inner.sent.record(scope, item, sha);
        }
        if let Err(e) = inner.sent.save(&self.sent_path) {
            inner.log.push_front(LogLine {
                at: now(),
                text: format!("Could not save upload state ({e})"),
            });
        }
        held.keys.len()
    }

    pub fn snapshot(&self, cfg: &AppConfig) -> CollectorStatusDto {
        let m = &WOW_LEDGERLINK;
        let inner = self.lock();
        let previews = inner
            .held
            .iter()
            .map(|(id, h)| (id.clone(), h.preview.clone()))
            .collect();
        let env = |k: &str| std::env::var(k).ok();
        CollectorStatusDto {
            enabled: cfg.collector_enabled(m.id, m.default_enabled),
            wow_dir: cfg.wow_dir.clone(),
            status: inner.status.clone(),
            results: inner.results.values().cloned().collect(),
            log: inner.log.iter().cloned().collect(),
            device: device_state(
                inner.paused,
                secs_left(inner.hold_until, Instant::now()),
                inner.linked.clone(),
            ),
            relink: inner.relink,
            collectors: rows(cfg, Platform::current(), &env, &inner.lines, &previews),
        }
    }
}

/// Whole seconds left on a hold at `now` (at least 1), `None` once it passed.
pub fn secs_left(until: Option<Instant>, now: Instant) -> Option<u64> {
    let left = until?.saturating_duration_since(now);
    if left.is_zero() {
        None
    } else {
        Some(left.as_secs().max(1))
    }
}

/// Whether a loop tick starts a scan: a file settled, the periodic rescan is
/// due, or a 429 hold just ran out. Nothing else wakes the loop when a hold
/// expires, so without the last case the batch sat until the next periodic
/// scan (ROK-2068).
pub fn tick_scans(was_held: bool, held: bool, settled: bool, idle_secs: u32) -> bool {
    settled || idle_secs >= PERIODIC_SCAN_SECS || (was_held && !held)
}

fn now() -> String {
    crate::wow_collector::captured_at(None)
}

/// Current status for the window: every collector's row and the device state.
pub fn status(app: &AppHandle, rt: &Runtime) -> CollectorStatusDto {
    rt.snapshot(&app.state::<Shared>().config())
}

fn publish(app: &AppHandle, rt: &Runtime, line: &str) {
    rt.set_status(line);
    tray::set_status(app, line);
    let _ = app.emit(STATUS_EVENT, status(app, rt));
}

/// Spawn the loop. Call once from `setup`.
pub fn start(app: AppHandle, rt: Arc<Runtime>) {
    tauri::async_runtime::spawn(async move { run_loop(app, rt).await });
}

/// Every enabled collector's configured roots: the watcher restarts when
/// this changes (so switching a collector on starts watching it).
fn roots_key(registry: &CollectorRegistry) -> Vec<PathBuf> {
    registry
        .entries()
        .iter()
        .filter(|e| e.enabled)
        .flat_map(|e| e.roots.iter().cloned())
        .collect()
}

async fn run_loop(app: AppHandle, rt: Arc<Runtime>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let mut watched: Option<Vec<PathBuf>> = None;
    let mut _watcher = None;
    let mut settle = SettleSet::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // After a long upload, don't burst missed ticks (that would fake the
    // settle samples on a file WoW is still writing).
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut idle_secs = 0u32;
    let mut was_held = false;
    scan_and_upload(&app, &rt).await;
    loop {
        let registry = CollectorRegistry::from_config(&app.state::<Shared>().config());
        let key = roots_key(&registry);
        if watched.as_ref() != Some(&key) {
            let (w, notes) = start_watcher(&registry.watch_roots(), tx.clone());
            registry
                .notes
                .iter()
                .chain(&notes)
                .for_each(|n| rt.log(n.clone()));
            _watcher = w;
            watched = Some(key);
        }
        tokio::select! {
            _ = tick.tick() => {
                idle_secs += 1;
                let held = rt.held_secs().is_some();
                let settled = !settle.tick().is_empty();
                if tick_scans(was_held, held, settled, idle_secs) {
                    idle_secs = 0;
                    scan_and_upload(&app, &rt).await;
                } else if held {
                    // Keep the window's rate-limit countdown live.
                    let _ = app.emit(STATUS_EVENT, status(&app, &rt));
                }
                was_held = held;
            }
            Some(path) = rx.recv() => {
                let samples = registry.wanting(&path).map(|e| e.manifest.settle_samples()).max();
                if let Some(samples) = samples {
                    settle.touch_with(path, samples);
                }
            }
            _ = rt.kick.notified() => {
                idle_secs = 0;
                scan_and_upload(&app, &rt).await;
            }
        }
    }
}

/// Scan every registered collector; upload what the enabled ones found.
async fn scan_and_upload(app: &AppHandle, rt: &Runtime) {
    rt.begin_scan();
    let cfg = app.state::<Shared>().config();
    let registry = CollectorRegistry::from_config(&cfg);
    let shared = app.state::<Shared>();
    let token = shared.token();
    let linked = token
        .as_ref()
        .map(Option::is_some)
        .map_err(|e| e.to_string());
    let paused = shared.is_paused();
    let held = rt.held_secs();
    let device = device_state(paused, held, linked.clone());
    let token = token.ok().flatten().filter(|_| device.is_none());
    let mut lines = BTreeMap::new();
    let mut relink = false;
    for entry in registry.entries() {
        if !entry.enabled {
            rt.set_held(entry.manifest.id, None);
            continue;
        }
        let (line, stop_all) = scan_and_upload_one(app, rt, &cfg, entry, token.as_ref()).await;
        lines.insert(entry.manifest.id.to_owned(), line);
        if stop_all {
            relink = true;
            break;
        }
    }
    let joined: Vec<String> = lines.values().cloned().collect();
    let line = status_line(&registry, &joined);
    let line = match device_note(paused, held.is_some(), linked.clone()) {
        Some(d) => format!("{d} · {line}"),
        None => line,
    };
    rt.set_device(paused, linked);
    rt.set_lines(lines, relink);
    publish(app, rt, &line);
}

/// The enabled collectors' lines; when none is on, say how to start.
pub fn status_line(registry: &CollectorRegistry, lines: &[String]) -> String {
    if !lines.is_empty() {
        return lines.join(" · ");
    }
    let names: Vec<&str> = registry
        .entries()
        .iter()
        .map(|e| e.manifest.display_name)
        .collect();
    format!("Uploads are off — turn on {} uploads", names.join(" or "))
}

/// One collector's pass. Returns its status line, and whether the device must
/// re-link (which stops every collector).
async fn scan_and_upload_one(
    app: &AppHandle,
    rt: &Runtime,
    cfg: &AppConfig,
    entry: &Entry,
    token: Option<&crate::token::DeviceToken>,
) -> (String, bool) {
    // `Entry::scan` is `None` for a disabled collector: nothing is read.
    let Some(scan) = entry.scan(&cfg.server_url, &rt.sent()) else {
        let name = entry.manifest.display_name;
        return (format!("{name} uploads are off"), false);
    };
    scan.notes.iter().for_each(|n| rt.log(n.clone()));
    let waiting = scan.pending.len();
    let id = entry.manifest.id;
    match gate(entry.manifest, cfg.consented(id), scan.files, &scan.pending) {
        Gate::NoFiles => {
            rt.set_held(id, None);
            return (entry.manifest.no_files_hint.to_owned(), false);
        }
        Gate::UpToDate => {
            rt.set_held(id, None);
            return ("Up to date".to_owned(), false);
        }
        Gate::Hold(held) => {
            rt.set_held(id, held.map(|h| *h));
            let line = format!("{waiting} waiting — check the preview before the first upload");
            return (line, false);
        }
        Gate::Send => rt.set_held(id, None),
    }
    // Device-level states (paused, not linked, rate limited) are shown once
    // by the window; here only the count.
    let Some(token) = token.filter(|_| rt.held_secs().is_none()) else {
        return (format!("{waiting} export(s) waiting"), false);
    };
    let uploader = match Uploader::new(&cfg.server_url) {
        Ok(u) => u,
        Err(e) => return (e, false),
    };
    publish(app, rt, &format!("Uploading {waiting} export(s)…"));
    upload_all(app, rt, &uploader, token, &scan.pending).await
}

async fn upload_all(
    app: &AppHandle,
    rt: &Runtime,
    uploader: &Uploader,
    token: &crate::token::DeviceToken,
    pending: &[PendingUpload],
) -> (String, bool) {
    let waiting = pending.len();
    let mut sent = 0usize;
    for p in pending {
        if app.state::<Shared>().is_paused() {
            return (
                format!("Paused — uploaded {sent} of {waiting} export(s)"),
                false,
            );
        }
        let outcome = send_with_blobs(uploader, token, p, tokio::time::sleep).await;
        let fx = effect(&outcome);
        rt.log(format!("{}: {}", p.label, fx.text));
        rt.finish(p, &fx);
        if fx.ok {
            sent += 1;
        }
        if let Some(wait) = fx.hold {
            rt.hold_for(wait);
        }
        if fx.relink {
            let _ = app.state::<Shared>().clear_token();
            return ("Re-link this device".to_owned(), true);
        }
        if fx.stop {
            let why = if fx.hold.is_some() {
                RETRYING
            } else {
                fx.text.as_str()
            };
            let line = format!("Uploaded {sent} of {waiting} export(s); {why}");
            return (line, false);
        }
    }
    (format!("Uploaded {sent} of {waiting} export(s)"), false)
}
