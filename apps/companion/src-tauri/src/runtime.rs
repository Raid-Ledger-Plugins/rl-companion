//! Collector runtime: watch → settle → scan → upload, plus the status the
//! window shows. One loop task per app; "Upload now" and setting changes
//! kick it through [`Runtime::kick`].

use crate::collector::{Collector, PendingUpload, SentState};
use crate::state::Shared;
use crate::tray;
use crate::upload::{UploadOutcome, Uploader};
use crate::watch::{start_watcher, SettleSet};
use crate::wow_collector::LedgerLinkCollector;
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

    pub fn snapshot(&self, enabled: bool, wow_dir: &str) -> CollectorStatusDto {
        let inner = self.lock();
        CollectorStatusDto {
            enabled,
            wow_dir: wow_dir.to_owned(),
            status: inner.status.clone(),
            results: inner.results.values().cloned().collect(),
            log: inner.log.iter().cloned().collect(),
        }
    }
}

fn now() -> String {
    crate::wow_collector::captured_at(None)
}

/// Current status for the window.
pub fn status(app: &AppHandle, rt: &Runtime) -> CollectorStatusDto {
    let cfg = app.state::<Shared>().config();
    rt.snapshot(cfg.ledgerlink_enabled, &cfg.wow_dir)
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

async fn run_loop(app: AppHandle, rt: Arc<Runtime>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let mut watched: Option<String> = None;
    let mut _watcher = None;
    let mut settle = SettleSet::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // After a long upload, don't burst missed ticks (that would fake the
    // settle samples on a file WoW is still writing).
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut idle_secs = 0u32;
    scan_and_upload(&app, &rt).await;
    loop {
        let dir = app.state::<Shared>().config().wow_dir;
        if watched.as_deref() != Some(dir.as_str()) {
            let (w, notes) =
                start_watcher(&LedgerLinkCollector::new(&dir).watch_roots(), tx.clone());
            notes.into_iter().for_each(|n| rt.log(n));
            _watcher = w;
            watched = Some(dir);
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
                if crate::wow_collector::is_ledgerlink_file(&path) {
                    settle.touch(path);
                }
            }
            _ = rt.kick.notified() => {
                idle_secs = 0;
                scan_and_upload(&app, &rt).await;
            }
        }
    }
}

async fn scan_and_upload(app: &AppHandle, rt: &Runtime) {
    let cfg = app.state::<Shared>().config();
    let collector = LedgerLinkCollector::new(&cfg.wow_dir);
    let scan = collector.scan(&cfg.server_url, &rt.sent());
    scan.notes.iter().for_each(|n| rt.log(n.clone()));
    let waiting = scan.pending.len();
    if scan.files == 0 {
        return publish(app, rt, "No LedgerLink.lua found — check the WoW folder");
    }
    if waiting == 0 {
        return publish(app, rt, "Up to date");
    }
    if !cfg.ledgerlink_enabled {
        return publish(
            app,
            rt,
            &format!("{waiting} export(s) waiting — turn on LedgerLink uploads"),
        );
    }
    if app.state::<Shared>().is_paused() {
        return publish(app, rt, &format!("Paused — {waiting} export(s) waiting"));
    }
    if let Some(secs) = rt.held_secs() {
        return publish(
            app,
            rt,
            &format!("Rate limited — {waiting} export(s) waiting, retry in {secs}s"),
        );
    }
    let token = match app.state::<Shared>().token() {
        Ok(Some(t)) => t,
        Ok(None) => return publish(app, rt, "Link this device to upload"),
        Err(e) => return publish(app, rt, &format!("Cannot read the device token ({e})")),
    };
    let uploader = match Uploader::new(&cfg.server_url) {
        Ok(u) => u,
        Err(e) => return publish(app, rt, &e),
    };
    publish(app, rt, &format!("Uploading {waiting} export(s)…"));
    let mut sent = 0usize;
    for pending in &scan.pending {
        if app.state::<Shared>().is_paused() {
            return publish(
                app,
                rt,
                &format!("Paused — uploaded {sent} of {waiting} export(s)"),
            );
        }
        let outcome = uploader
            .send(&token, pending.contract, &pending.body, tokio::time::sleep)
            .await;
        let fx = effect(&outcome);
        rt.log(format!("{}: {}", pending.label, fx.text));
        rt.finish(pending, &fx);
        if fx.ok {
            sent += 1;
        }
        if let Some(wait) = fx.hold {
            rt.hold_for(wait);
        }
        if fx.relink {
            let _ = app.state::<Shared>().clear_token();
            return publish(app, rt, "Re-link this device");
        }
        if fx.stop {
            let line = format!("Uploaded {sent} of {waiting} export(s); {}", fx.text);
            return publish(app, rt, &line);
        }
    }
    publish(app, rt, &format!("Uploaded {sent} of {waiting} export(s)"));
}
