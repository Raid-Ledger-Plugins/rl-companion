//! The probe store (ROK-2191): every submitted probe, persisted at
//! `<app_data_dir>/dev-bridge/probes.json`, and the per-flavor DevQueue.lua
//! it keeps in step. A probe is stored only once its queue file was written;
//! every state change rewrites the affected flavor's queue.

use super::probe_flavor::{resolve_flavor, FlavorError};
use super::probe_id::new_probe_id;
use super::probe_queue::{render, QUEUE_MAX};
use super::probe_record::{clip_error, ProbeRecord, ProbeStatus};
use super::probe_request::ProbeRequest;
use super::write_guard::{guarded_write, queue_path, write_atomic, WriteGuardError};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub const STORE_FILE: [&str; 2] = ["dev-bridge", "probes.json"];
/// Records kept; the oldest finished ones are dropped first.
pub const STORE_MAX: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum ProbeStoreError {
    #[error(transparent)]
    Flavor(#[from] FlavorError),
    /// Already [`QUEUE_MAX`] live probes for this flavor. The contract names no
    /// error for this; the HTTP layer answers `invalid_request` at `[]`, `custom`.
    #[error("the {0} probe queue is full ({QUEUE_MAX} pending) — cancel one or wait")]
    QueueFull(String),
    #[error(transparent)]
    Write(#[from] WriteGuardError),
    #[error("could not save the probe store: {0}")]
    Persist(std::io::Error),
    #[error("no probe {0}")]
    NotFound(String),
    #[error("probe {0} is {1}, not queued")]
    NotQueued(String, &'static str),
}

#[derive(Debug)]
pub struct ProbeStore {
    path: PathBuf,
    records: Mutex<Vec<ProbeRecord>>,
}

impl ProbeStore {
    /// The store under the app data dir.
    pub fn in_app_data(app_data_dir: &Path) -> Self {
        Self::open(
            STORE_FILE
                .iter()
                .fold(app_data_dir.to_path_buf(), |p, s| p.join(s)),
        )
    }

    /// Load `path`; a missing or corrupt file starts empty.
    pub fn open(path: PathBuf) -> Self {
        let records = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<ProbeRecord>>(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            records: Mutex::new(records),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<ProbeRecord>> {
        self.records.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Queue `req`: resolve its flavor, write that flavor's DevQueue.lua with
    /// it, then save. Any failure leaves neither the record nor the entry.
    pub fn submit(
        &self,
        wow_dir: &Path,
        req: &ProbeRequest,
        now: i64,
    ) -> Result<ProbeRecord, ProbeStoreError> {
        let flavor = resolve_flavor(wow_dir, req)?;
        let mut recs = self.lock();
        let live = recs.iter().filter(|r| r.flavor == flavor && r.is_live(now));
        if live.count() >= QUEUE_MAX {
            return Err(ProbeStoreError::QueueFull(flavor));
        }
        let rec = ProbeRecord {
            probe_id: new_probe_id(),
            title: req.title.clone(),
            status: ProbeStatus::Queued,
            submitted_at: now,
            answered_at: None,
            error: None,
            flavor: flavor.clone(),
            ttl_hours: req.ttl_hours,
            request: req.echo_json(&flavor),
            result: None,
        };
        recs.push(rec.clone());
        let saved = write_queue(wow_dir, &recs, &flavor, now)
            .and_then(|()| self.save(&recs).map_err(ProbeStoreError::Persist));
        if let Err(e) = saved {
            recs.pop();
            let _ = write_queue(wow_dir, &recs, &flavor, now);
            return Err(e);
        }
        prune(&mut recs);
        let _ = self.save(&recs);
        Ok(rec)
    }

    /// DELETE: a queued probe → `cancelled`, and off its queue file.
    pub fn cancel(
        &self,
        wow_dir: &Path,
        id: &str,
        now: i64,
    ) -> Result<ProbeRecord, ProbeStoreError> {
        let mut recs = self.lock();
        let i = queued_index(&recs, id)?;
        recs[i].status = ProbeStatus::Cancelled;
        let flavor = recs[i].flavor.clone();
        if let Err(e) = write_queue(wow_dir, &recs, &flavor, now) {
            recs[i].status = ProbeStatus::Queued;
            return Err(e);
        }
        // Unsaved, a restart would bring it back as queued: undo instead.
        if let Err(e) = self.save(&recs) {
            recs[i].status = ProbeStatus::Queued;
            let _ = write_queue(wow_dir, &recs, &flavor, now);
            return Err(ProbeStoreError::Persist(e));
        }
        Ok(recs[i].clone())
    }

    /// Every queued probe past its ttl → `expired`; their queues rewritten.
    /// Returns the expired ids.
    pub fn expire_due(&self, wow_dir: &Path, now: i64) -> Vec<String> {
        let mut recs = self.lock();
        let mut ids = Vec::new();
        let mut flavors = Vec::new();
        for r in recs.iter_mut().filter(|r| r.is_due(now)) {
            r.status = ProbeStatus::Expired;
            ids.push(r.probe_id.clone());
            if !flavors.contains(&r.flavor) {
                flavors.push(r.flavor.clone());
            }
        }
        if !ids.is_empty() {
            for f in &flavors {
                let _ = write_queue(wow_dir, &recs, f, now);
            }
            let _ = self.save(&recs);
        }
        ids
    }

    /// LedgerLink answered: `Ok(result)` → `answered`, `Err(why)` →
    /// `rejected` (the result failed `probe-result`). Only from `queued`.
    pub fn apply_answer(
        &self,
        wow_dir: &Path,
        id: &str,
        outcome: Result<Value, String>,
        now: i64,
    ) -> Result<ProbeRecord, ProbeStoreError> {
        let mut recs = self.lock();
        let i = queued_index(&recs, id)?;
        let r = &mut recs[i];
        r.answered_at = Some(now);
        match outcome {
            Ok(v) => (r.status, r.result) = (ProbeStatus::Answered, Some(v)),
            Err(e) => (r.status, r.error) = (ProbeStatus::Rejected, Some(clip_error(&e))),
        }
        let flavor = r.flavor.clone();
        // The answer is the truth even if the stale entry stays in the file.
        let _ = write_queue(wow_dir, &recs, &flavor, now);
        let _ = self.save(&recs);
        Ok(recs[i].clone())
    }

    /// Every record (oldest first), optionally of one status.
    pub fn list(&self, status: Option<ProbeStatus>) -> Vec<ProbeRecord> {
        let recs = self.lock();
        recs.iter()
            .filter(|r| status.is_none_or(|s| r.status == s))
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<ProbeRecord> {
        self.lock().iter().find(|r| r.probe_id == id).cloned()
    }

    fn save(&self, recs: &[ProbeRecord]) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec(recs).map_err(std::io::Error::other)?;
        write_atomic(&self.path, &bytes)
    }
}

fn queued_index(recs: &[ProbeRecord], id: &str) -> Result<usize, ProbeStoreError> {
    let i = recs
        .iter()
        .position(|r| r.probe_id == id)
        .ok_or_else(|| ProbeStoreError::NotFound(id.to_owned()))?;
    match recs[i].status {
        ProbeStatus::Queued => Ok(i),
        s => Err(ProbeStoreError::NotQueued(id.to_owned(), s.as_str())),
    }
}

/// Rewrite `flavor`'s DevQueue.lua: its live probes, oldest first, ≤ 50.
pub fn write_queue(
    wow_dir: &Path,
    recs: &[ProbeRecord],
    flavor: &str,
    now: i64,
) -> Result<(), ProbeStoreError> {
    let entries: Vec<Value> = recs
        .iter()
        .filter(|r| r.flavor == flavor && r.is_live(now))
        .take(QUEUE_MAX)
        .map(ProbeRecord::queue_entry)
        .collect();
    let target = queue_path(wow_dir, flavor)?;
    Ok(guarded_write(
        wow_dir,
        &target,
        render(&entries).as_bytes(),
    )?)
}

/// Keep at most [`STORE_MAX`] records, dropping the oldest finished first.
fn prune(recs: &mut Vec<ProbeRecord>) {
    while recs.len() > STORE_MAX {
        match recs.iter().position(|r| r.status != ProbeStatus::Queued) {
            Some(i) => {
                recs.remove(i);
            }
            None => break,
        }
    }
}
