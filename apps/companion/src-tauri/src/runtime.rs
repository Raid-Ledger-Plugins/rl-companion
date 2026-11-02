//! Collector runtime: watch → settle → scan → upload, plus the status the
//! window shows. One loop task per app; "Upload now" and setting changes
//! kick it through [`Runtime::kick`].

use crate::collector::{PendingUpload, SentState};
use crate::collector_view::{device_state, rows, CollectorRowDto};
use crate::config::AppConfig;
use crate::manifest::{Platform, WOW_LEDGERLINK};
use crate::preview::{hold, Held};
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
            let text = format!("Rate limited; will retry in {}s", wait.as_secs());
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
    results: BTreeMap<String, CharResult>,
    log: VecDeque<LogLine>,
    sent: SentState,
    /// No uploads before this (a 429's `Retry-After`).
    hold_until: Option<Instant>,
    /// Each collector's own line from the last scan.
    lines: BTreeMap<String, String>,
    device: Option<String>,
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
                device: None,
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
        let until = self.lock().hold_until?;
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            None
        } else {
            Some(left.as_secs().max(1))
        }
    }

    pub fn set_status(&self, text: impl Into<String>) {
        self.lock().status = text.into();
    }

    /// Settle one upload: remember the result, and the sha if `record`.
    pub fn finish(&self, pending: &PendingUpload, fx: &Effect) {
        let mut inner = self.lock();
        let at = now();
        inner.results.insert(
            pending.item.clone(),
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

    /// Remember the scan's per-collector lines and the device state.
    pub fn set_lines(&self, lines: BTreeMap<String, String>, device: Option<String>) {
        let mut inner = self.lock();
        inner.lines = lines;
        inner.device = device;
    }

    /// Hold `id`'s batch behind its first-upload preview (replaces any
    /// earlier hold); `None` clears it.
    pub fn set_held(&self, id: &str, held: Option<Held>) {
        let mut inner = self.lock();
        match held {
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
            device: inner.device.clone(),
            collectors: rows(cfg, Platform::current(), &env, &inner.lines, &previews),
        }
    }
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
                if !settle.tick().is_empty() || idle_secs >= PERIODIC_SCAN_SECS {
                    idle_secs = 0;
                    scan_and_upload(&app, &rt).await;
                }
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
    let cfg = app.state::<Shared>().config();
    let registry = CollectorRegistry::from_config(&cfg);
    let shared = app.state::<Shared>();
    let token = shared.token();
    let linked = token
        .as_ref()
        .map(Option::is_some)
        .map_err(|e| e.to_string());
    let device = device_state(shared.is_paused(), rt.held_secs(), linked);
    let token = token.ok().flatten().filter(|_| device.is_none());
    let mut lines = BTreeMap::new();
    for entry in registry.entries() {
        if !entry.enabled {
            rt.set_held(entry.manifest.id, None);
            continue;
        }
        let (line, stop_all) = scan_and_upload_one(app, rt, &cfg, entry, token.as_ref()).await;
        lines.insert(entry.manifest.id.to_owned(), line);
        if stop_all {
            break;
        }
    }
    let joined: Vec<String> = lines.values().cloned().collect();
    let line = status_line(&registry, &joined);
    let line = match &device {
        Some(d) => format!("{d} · {line}"),
        None => line,
    };
    rt.set_lines(lines, device);
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
    if scan.files == 0 {
        return (entry.manifest.no_files_hint.to_owned(), false);
    }
    let id = entry.manifest.id;
    if waiting == 0 {
        rt.set_held(id, None);
        return ("Up to date".to_owned(), false);
    }
    // First upload of a confirm-first collector: hold it behind a local
    // preview until the user says Upload (consent) or Skip.
    if entry.manifest.confirm_first_upload && !cfg.consented(id) {
        rt.set_held(id, hold(id, &scan.pending));
        let line = format!("{waiting} waiting — check the preview before the first upload");
        return (line, false);
    }
    rt.set_held(id, None);
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

/// PUT the message's blobs, then POST it. A blob the server refuses
/// terminally is skipped (the message is still useful without it); any other
/// blob failure (401, 429, network) is the message's outcome.
async fn send_with_blobs(
    uploader: &Uploader,
    token: &crate::token::DeviceToken,
    p: &PendingUpload,
) -> UploadOutcome {
    for blob in &p.blobs {
        match uploader.send_blob(token, blob, tokio::time::sleep).await {
            UploadOutcome::Accepted | UploadOutcome::Rejected { .. } => {}
            other => return other,
        }
    }
    uploader
        .send(token, p.contract, &p.body, tokio::time::sleep)
        .await
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
        let outcome = send_with_blobs(uploader, token, p).await;
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
            let line = format!("Uploaded {sent} of {waiting} export(s); {}", fx.text);
            return (line, false);
        }
    }
    (format!("Uploaded {sent} of {waiting} export(s)"), false)
}
