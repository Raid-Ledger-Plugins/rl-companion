//! File watching: a `notify` watcher feeds changed paths in, and
//! [`SettleSet`] holds each one until WoW has finished writing it (the crate's
//! [`SettleWatcher`] over successive `(size, mtime)` samples).

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use rl_collectors::{FileSample, SettleWatcher, DEFAULT_STABLE_SAMPLES};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tokio::sync::mpsc::UnboundedSender;

/// One `(size, mtime)` sample; `None` if the file is missing.
pub fn sample(path: &Path) -> Option<FileSample> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Some(FileSample {
        size: meta.len(),
        mtime_ms,
    })
}

/// Paths waiting to settle. Sample them on a fixed tick (1 s in the app).
#[derive(Debug, Default)]
pub struct SettleSet {
    pending: HashMap<PathBuf, SettleWatcher>,
}

impl SettleSet {
    /// A change was seen; start (or keep) watching the path.
    pub fn touch(&mut self, path: PathBuf) {
        self.pending
            .entry(path)
            .or_insert_with(|| SettleWatcher::new(DEFAULT_STABLE_SAMPLES));
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Sample every pending path; return (and forget) the ones that settled.
    pub fn tick(&mut self) -> Vec<PathBuf> {
        let mut done = Vec::new();
        self.pending.retain(|path, watcher| {
            if watcher.observe(sample(path)) {
                done.push(path.clone());
                false
            } else {
                true
            }
        });
        done.sort();
        done
    }
}

/// Watch `roots` recursively, sending every changed path to `tx`. Roots that
/// cannot be watched are returned as notes; the periodic rescan covers them.
pub fn start_watcher(
    roots: &[PathBuf],
    tx: UnboundedSender<PathBuf>,
) -> (Option<RecommendedWatcher>, Vec<String>) {
    let handler = move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    };
    let mut watcher = match notify::recommended_watcher(handler) {
        Ok(w) => w,
        Err(e) => return (None, vec![format!("File watching unavailable ({e}); rescanning every few minutes")]),
    };
    let mut notes = Vec::new();
    for root in roots {
        if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
            notes.push(format!("Cannot watch {} ({e})", root.display()));
        }
    }
    (Some(watcher), notes)
}
