//! Collector runtime: watch → settle → scan → upload, plus the status the
//! window shows. One loop task per app; "Upload now" and setting changes
//! kick it through [`Runtime::kick`].

use crate::blob_send::send_with_blobs;
use crate::collector::{PendingUpload, SentState};
use crate::collector_status::{device_block, finished_status, Line, LineStatus};
use crate::collector_view::{device_note, device_state, rows, CollectorRowDto, RETRYING};
use crate::config::AppConfig;
use crate::gate::{gate, Gate};
use crate::manifest::{Platform, WOW_LEDGERLINK};
use crate::preview::Held;
use crate::registry::{CollectorRegistry, Entry};
use crate::scan_report::{self, ScanBook, ScanPass, ScanReport, UploadTimes};
use crate::server_config::ServerState;
use crate::state::Shared;
use crate::tiles::{apply_plugin_off_copy, last_checked, tiles, TileDto, TileInputs};
use crate::tray;
use crate::upload::{is_unknown_contract, UploadOutcome, Uploader};
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
    /// The collector that produced it (ROK-2067 D12): each game view shows
    /// only its own results.
    pub collector_id: String,
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
    /// The collector it is about; `None` = a device-level line (D12).
    pub collector_id: Option<String>,
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
    /// One tile per game (ROK-2067 D6/D7).
    pub tiles: Vec<TileDto>,
    /// When the cached `/config` was fetched, set only while the server is
    /// unreachable and a cache is shown (D7 "Last checked").
    pub last_checked: Option<String>,
    /// Per-collector scan/upload facts for `/dev/v1/collectors` (not sent
    /// to the window).
    #[serde(skip)]
    pub scan: BTreeMap<String, ScanReport>,
    /// Each collector's status from the last scan (the line's
    /// `collectors[].status`; not sent to the window).
    #[serde(skip)]
    pub line_status: BTreeMap<String, LineStatus>,
    /// The status of [`Self::device`]'s state (same inputs, same precedence).
    #[serde(skip)]
    pub device_status: Option<LineStatus>,
    /// Whether the device holds a token; `None` when it cannot be read.
    #[serde(skip)]
    pub linked: Option<bool>,
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

/// A screenshot refused because the hub's Gallery is full (bg3-screenshot/v1 §3).
pub const GALLERY_FULL_TEXT: &str =
    "Not uploaded: the hub's Gallery is full — a hub leader can free space";

/// Only accepted uploads and the server's content rejections are recorded;
/// everything else is tried again on a later scan.
pub fn effect(outcome: &UploadOutcome) -> Effect {
    match outcome {
        UploadOutcome::Accepted => settle(true, false, true, "Uploaded".to_owned()),
        UploadOutcome::Rejected { code, .. } if code == crate::upload::GALLERY_FULL_CODE => {
            // Settled (never resent), and the batch stops: the rest would hit
            // the same full Gallery and stay queued for a later scan.
            settle(true, true, false, GALLERY_FULL_TEXT.to_owned())
        }
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
    /// Keyed by `(collector_id, scope, item)`: every BG3 save shares the item
    /// `save`, so the scope (one per save file) keeps one save's result from
    /// overwriting another's, and the collector keeps games apart (D12).
    results: BTreeMap<(String, String, String), CharResult>,
    log: VecDeque<LogLine>,
    sent: SentState,
    /// No uploads before this (a 429's `Retry-After`).
    hold_until: Option<Instant>,
    /// Each collector's own line from the last scan.
    lines: BTreeMap<String, String>,
    /// The status each of those lines stands for (dev-bridge `status`).
    statuses: BTreeMap<String, LineStatus>,
    /// The last scan's device inputs; the window's device line is derived
    /// from them and the live hold at render time (ROK-2068).
    paused: bool,
    linked: Result<bool, String>,
    /// The last scan hit a 401 and cleared the token.
    relink: bool,
    /// Batches waiting on the user's first-upload confirmation.
    held: BTreeMap<String, Held>,
    /// `/config` fetch state and cached covers (ROK-2067).
    server: ServerState,
    /// Last scan, unsettled count and last accepted upload per collector.
    book: ScanBook,
}

pub struct Runtime {
    sent_path: PathBuf,
    inner: Mutex<Inner>,
    kick: Notify,
    /// Wakes the `/config` refresh task (after link, on a deferral).
    pub config_kick: Notify,
}

impl Runtime {
    pub fn new(sent_path: PathBuf) -> Self {
        let sent = SentState::load(&sent_path);
        let uploads = UploadTimes::load(&scan_report::upload_times_path(&sent_path));
        Self {
            sent_path,
            inner: Mutex::new(Inner {
                status: "Starting…".into(),
                results: BTreeMap::new(),
                log: VecDeque::new(),
                sent,
                hold_until: None,
                lines: BTreeMap::new(),
                statuses: BTreeMap::new(),
                paused: false,
                linked: Ok(true),
                relink: false,
                held: BTreeMap::new(),
                server: ServerState::default(),
                book: ScanBook::new(uploads),
            }),
            kick: Notify::new(),
            config_kick: Notify::new(),
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

    /// A device-level activity line.
    pub fn log(&self, text: impl Into<String>) {
        self.push_log(None, text.into());
    }

    /// An activity line about one collector (D12).
    pub fn log_for(&self, collector_id: &str, text: impl Into<String>) {
        self.push_log(Some(collector_id.to_owned()), text.into());
    }

    /// Into the window's Activity log only. Its free text (paths, character,
    /// save and campaign names) is never mirrored into the dev-bridge ring.
    fn push_log(&self, collector_id: Option<String>, text: String) {
        let mut inner = self.lock();
        inner.log.push_front(LogLine {
            at: now(),
            text,
            collector_id,
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

    /// `true` exactly once after a 429 hold ran out, clearing it. The expiry
    /// is latched on the instant, not sampled per tick, so a hold that runs
    /// out while a scan is still running still triggers the next scan.
    pub fn take_expired_hold(&self) -> bool {
        take_expired(&mut self.lock().hold_until, Instant::now())
    }

    pub fn set_status(&self, text: impl Into<String>) {
        self.lock().status = text.into();
    }

    /// Settle one upload: remember the result, and the sha if `record`.
    pub fn finish(&self, pending: &PendingUpload, fx: &Effect) {
        let mut inner = self.lock();
        let at = now();
        let collector_id = pending.contract.to_owned();
        inner.results.insert(
            (
                collector_id.clone(),
                pending.scope.clone(),
                pending.item.clone(),
            ),
            CharResult {
                collector_id,
                label: pending.label.clone(),
                result: shown_text(fx).to_owned(),
                ok: fx.ok,
                at: at.clone(),
            },
        );
        // Saved after the guard is dropped: no file IO under the lock.
        let uploads = fx.ok.then(|| {
            inner.book.uploaded(pending.contract, at);
            inner.book.uploads.clone()
        });
        if fx.record {
            inner
                .sent
                .record(&pending.scope, &pending.item, &pending.sha256);
            if let Err(e) = inner.sent.save(&self.sent_path) {
                inner.log.push_front(LogLine {
                    at: now(),
                    text: format!("Could not save upload state ({e})"),
                    collector_id: None,
                });
            }
        }
        drop(inner);
        let path = scan_report::upload_times_path(&self.sent_path);
        if let Some(Err(e)) = uploads.map(|u| u.save(&path)) {
            self.log(format!("Could not save upload times ({e})"));
        }
    }

    /// A scan of `id` ran: stamp it, with what it left unsettled — its
    /// queued items the server has not settled, plus `waiting` items it could
    /// not queue yet (screenshots waiting for a hub).
    pub fn scanned(&self, id: &str, pending: &[PendingUpload], waiting: usize) {
        let mut inner = self.lock();
        let left = scan_report::unsettled(&inner.sent, pending) + waiting;
        inner.book.scanned(id, now(), left);
    }

    /// `id` is switched off, so it reports no scan.
    pub fn forget_scan(&self, id: &str) {
        self.lock().book.forget_scan(id);
    }

    /// Remember the scan's per-collector lines and the 401 flag.
    pub fn set_lines(&self, lines: BTreeMap<String, String>, relink: bool) {
        let mut inner = self.lock();
        inner.lines = lines;
        inner.relink = relink;
    }

    /// Remember the statuses of the scan's per-collector lines.
    pub fn set_statuses(&self, statuses: BTreeMap<String, LineStatus>) {
        self.lock().statuses = statuses;
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
    /// scan runs now so the waiting exports go up with the new token. The
    /// server's `/config` is fetched again too (ROK-2067 D5).
    pub fn linked(&self) {
        self.lock().relink = false;
        self.kick();
        self.config_kick.notify_one();
    }

    /// The server URL changed: forget the old server's reachability and
    /// covers, take the new server's link state (`linked` = it has a token)
    /// without waiting for a scan, and fetch its `/config` now (ROK-2067 D5).
    pub fn server_changed(&self, linked: bool) {
        let mut inner = self.lock();
        inner.server.reset();
        inner.linked = Ok(linked);
        inner.relink = false;
        drop(inner);
        self.config_kick.notify_one();
    }

    /// `/config` answered 401: the token was cleared, so the tiles show the
    /// unlinked state and the window asks for a re-link.
    pub fn config_unauthorized(&self) {
        let mut inner = self.lock();
        inner.relink = true;
        inner.linked = Ok(false);
    }

    /// The `/config` fetch state (reachability, covers, debounce).
    pub fn server(&self) -> ServerState {
        self.lock().server.clone()
    }

    pub fn update_server<R>(&self, f: impl FnOnce(&mut ServerState) -> R) -> R {
        f(&mut self.lock().server)
    }

    /// An upload hit `unknown_contract`: refetch `/config`, at most once a
    /// minute (D5). Returns whether a refresh was requested.
    pub fn deferred_unknown_contract(&self) -> bool {
        let due = self.lock().server.take_defer_slot(Instant::now());
        if due {
            self.config_kick.notify_one();
        }
        due
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
                collector_id: None,
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
        let mut collectors = rows(cfg, Platform::current(), &env, &inner.lines, &previews);
        let mut results: Vec<CharResult> = inner.results.values().cloned().collect();
        let linked = matches!(inner.linked, Ok(true));
        let held = secs_left(inner.hold_until, Instant::now());
        apply_plugin_off_copy(cfg, linked, &mut collectors, &mut results);
        let view = TileInputs {
            cfg,
            linked,
            rows: &collectors,
            covers: &inner.server.covers,
        };
        CollectorStatusDto {
            enabled: cfg.collector_enabled(m.id, m.default_enabled),
            wow_dir: cfg.wow_dir.clone(),
            status: inner.status.clone(),
            results,
            log: inner.log.iter().cloned().collect(),
            device: device_state(inner.paused, held, inner.linked.clone()),
            device_status: device_block(inner.paused, held, &inner.linked).map(|b| b.status()),
            linked: inner.linked.as_ref().ok().copied(),
            line_status: inner.statuses.clone(),
            relink: inner.relink,
            tiles: tiles(&view),
            last_checked: last_checked(cfg, linked, inner.server.reachable),
            scan: inner.book.reports(),
            collectors,
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

/// Clear `until` and return `true` if it is set and has passed at `now`.
pub fn take_expired(until: &mut Option<Instant>, now: Instant) -> bool {
    let expired = until.is_some() && secs_left(*until, now).is_none();
    if expired {
        *until = None;
    }
    expired
}

/// Whether a loop tick starts a scan: a file settled, the periodic rescan is
/// due, or a 429 hold ran out (from [`Runtime::take_expired_hold`]). Nothing
/// else wakes the loop when a hold expires, so without the last case the
/// batch sat until the next periodic scan (ROK-2068).
pub fn tick_scans(hold_expired: bool, settled: bool, idle_secs: u32) -> bool {
    settled || idle_secs >= PERIODIC_SCAN_SECS || hold_expired
}

/// The copy a settled upload leaves on screen. A 429's text carries a
/// countdown that would freeze there, so it shows [`RETRYING`] instead.
pub fn shown_text(fx: &Effect) -> &str {
    if fx.hold.is_some() {
        RETRYING
    } else {
        fx.text.as_str()
    }
}

/// The batch line when an upload stopped the batch.
pub fn stopped_line(sent: usize, waiting: usize, fx: &Effect) -> String {
    format!("Uploaded {sent} of {waiting} export(s); {}", shown_text(fx))
}

/// What stopped the batch, as a status: 401 → `link-device`, 429 →
/// `rate-limited`, a settled refusal (a full Gallery) → `rejected`, a
/// deferral → `retrying`.
pub fn stop_status(fx: &Effect) -> LineStatus {
    if fx.relink {
        LineStatus::LinkDevice
    } else if fx.hold.is_some() {
        LineStatus::RateLimited
    } else if fx.record {
        LineStatus::Rejected
    } else {
        LineStatus::Retrying
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
                let hold_expired = rt.take_expired_hold();
                let settled = !settle.tick().is_empty();
                if tick_scans(hold_expired, settled, idle_secs) {
                    idle_secs = 0;
                    scan_and_upload(&app, &rt).await;
                } else if rt.held_secs().is_some() {
                    // Keep the window's rate-limit countdown live.
                    let _ = app.emit(STATUS_EVENT, status(&app, &rt));
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
            rt.forget_scan(entry.manifest.id);
            rt.set_held(entry.manifest.id, None);
            if entry.manifest.id == crate::manifest::BG3_SCREENSHOT.id {
                crate::bg3_screenshot::clear_waiting();
            }
            continue;
        }
        let (line, stop_all) = scan_and_upload_one(app, rt, &cfg, entry, token.as_ref()).await;
        lines.insert(entry.manifest.id.to_owned(), line);
        if stop_all {
            relink = true;
            break;
        }
    }
    crate::bg3_campaigns::persist_first_sight(&crate::bg3_campaigns::STORE, &shared);
    let statuses = lines.iter().map(|(k, l)| (k.clone(), l.status)).collect();
    let lines: BTreeMap<String, String> = lines.into_iter().map(|(k, l)| (k, l.text)).collect();
    let joined: Vec<String> = lines.values().cloned().collect();
    let line = status_line(&registry, &joined);
    let line = match device_note(paused, held.is_some(), linked.clone()) {
        Some(d) => format!("{d} · {line}"),
        None => line,
    };
    rt.set_device(paused, linked);
    rt.set_lines(lines, relink);
    rt.set_statuses(statuses);
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
/// re-link (which stops every collector). A pass that read files stamps the
/// collector's scan time and what it left unsettled.
async fn scan_and_upload_one(
    app: &AppHandle,
    rt: &Runtime,
    cfg: &AppConfig,
    entry: &Entry,
    token: Option<&crate::token::DeviceToken>,
) -> (Line, bool) {
    // `None` for a disabled collector: nothing is read. Dropping the pass
    // at the end stamps the scan, whichever way the upload returned.
    let Some(mut pass) = ScanPass::begin(rt, entry, &cfg.server_url) else {
        let name = entry.manifest.display_name;
        let text = format!("{name} uploads are off");
        return (Line::new(LineStatus::Off, text), false);
    };
    gate_and_upload(app, rt, cfg, entry, token, &mut pass.scan).await
}

async fn gate_and_upload(
    app: &AppHandle,
    rt: &Runtime,
    cfg: &AppConfig,
    entry: &Entry,
    token: Option<&crate::token::DeviceToken>,
    scan: &mut crate::collector::Scan,
) -> (Line, bool) {
    scan.notes
        .iter()
        .for_each(|n| rt.log_for(entry.manifest.id, n.clone()));
    // A held screenshot batch's preview image is downscaled off the runtime.
    crate::screenshot_preview::attach(scan).await;
    let waiting = scan.pending.len();
    let id = entry.manifest.id;
    match gate(entry.manifest, cfg.consented(id), scan.files, &scan.pending) {
        Gate::NoFiles => {
            rt.set_held(id, None);
            let hint = entry.manifest.no_files_hint;
            return (Line::new(LineStatus::NoFiles, hint), false);
        }
        Gate::UpToDate => {
            rt.set_held(id, None);
            return (Line::new(LineStatus::UpToDate, "Up to date"), false);
        }
        Gate::Hold(held) => {
            rt.set_held(id, held.map(|h| *h));
            let line = format!("{waiting} waiting — check the preview before the first upload");
            return (Line::new(LineStatus::NeedsConfirm, line), false);
        }
        Gate::Send => rt.set_held(id, None),
    }
    // Device-level states (paused, not linked, rate limited) are shown once
    // by the window; here only the count.
    let Some(token) = token.filter(|_| rt.held_secs().is_none()) else {
        let text = format!("{waiting} export(s) waiting");
        return (Line::new(LineStatus::Waiting, text), false);
    };
    let uploader = match Uploader::new(&cfg.server_url) {
        Ok(u) => u,
        Err(e) => return (Line::new(LineStatus::Error, e), false),
    };
    publish(app, rt, &format!("Uploading {waiting} export(s)…"));
    upload_all(app, rt, &uploader, token, cfg, &scan.pending).await
}

/// A screenshot is read and sent only while its collector is still switched
/// on with the opt-in the batch was scanned under (bg3-screenshot consent).
fn switched_off(app: &AppHandle, scanned: &AppConfig, p: &PendingUpload) -> bool {
    p.contract == crate::bg3_screenshot::CONTRACT
        && !crate::bg3_screenshot::still_opted_in(scanned, &app.state::<Shared>().config())
}

fn off_line(sent: usize, waiting: usize) -> String {
    format!("Uploads switched off — uploaded {sent} of {waiting} export(s)")
}

async fn upload_all(
    app: &AppHandle,
    rt: &Runtime,
    uploader: &Uploader,
    token: &crate::token::DeviceToken,
    scanned: &AppConfig,
    pending: &[PendingUpload],
) -> (Line, bool) {
    let waiting = pending.len();
    let mut sent = 0usize;
    let mut settled = 0usize;
    for p in crate::bg3_campaigns::still_ticked(pending, app) {
        if app.state::<Shared>().is_paused() {
            let text = format!("Paused — uploaded {sent} of {waiting} export(s)");
            return (Line::new(LineStatus::Paused, text), false);
        }
        // Before the original is read …
        if switched_off(app, scanned, p) {
            return (Line::new(LineStatus::Off, off_line(sent, waiting)), false);
        }
        let ready;
        let p = if crate::bg3_screenshot::needs_encode(p) {
            // Decode + WebP encode off the async runtime; a crash refuses
            // only this file and the loop carries on.
            match crate::bg3_screenshot::encode_for_send(p).await {
                Ok(done) => {
                    ready = done;
                    &ready
                }
                Err(line) => {
                    rt.log_for(p.contract, format!("{}: {line}", p.label));
                    continue;
                }
            }
        } else {
            p
        };
        // … and again before it is sent (the encode may have taken a while).
        if switched_off(app, scanned, p) {
            return (Line::new(LineStatus::Off, off_line(sent, waiting)), false);
        }
        let outcome = send_with_blobs(uploader, token, p, tokio::time::sleep).await;
        let fx = effect(&outcome);
        rt.log_for(p.contract, format!("{}: {}", p.label, fx.text));
        if is_unknown_contract(&outcome) {
            rt.deferred_unknown_contract();
        }
        rt.finish(p, &fx);
        if fx.ok {
            sent += 1;
        }
        if fx.record {
            settled += 1;
        }
        if let Some(wait) = fx.hold {
            rt.hold_for(wait);
        }
        if fx.relink {
            let _ = app.state::<Shared>().clear_token();
            return (
                Line::new(LineStatus::LinkDevice, "Re-link this device"),
                true,
            );
        }
        if fx.stop {
            let text = stopped_line(sent, waiting, &fx);
            return (Line::new(stop_status(&fx), text), false);
        }
    }
    let text = format!("Uploaded {sent} of {waiting} export(s)");
    let status = finished_status(sent, waiting.saturating_sub(settled));
    (Line::new(status, text), false)
}
