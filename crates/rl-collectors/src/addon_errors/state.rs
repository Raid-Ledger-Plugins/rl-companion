//! Per-file dedupe state: an unchanged file yields nothing; only new
//! records, or records whose `count` grew, are emitted.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{AddonErrorRecord, BugGrabberScan};

/// Persist one per `!BugGrabber.lua` path (the caller owns storage).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonErrorsState {
    /// SHA-256 of the file whose records were all accepted.
    pub file_sha256: Option<String>,
    /// Record key → the `count` last accepted by the server.
    pub uploaded: BTreeMap<String, u32>,
}

impl AddonErrorsState {
    /// The file is byte-identical to the last fully uploaded one.
    pub fn is_unchanged(&self, scan: &BugGrabberScan) -> bool {
        self.file_sha256.as_deref() == Some(scan.file_sha256.as_str())
    }

    /// Records to upload: none for an unchanged file; otherwise records never
    /// accepted, or whose `count` is higher than the accepted one.
    pub fn pending(&self, scan: &BugGrabberScan) -> Vec<AddonErrorRecord> {
        if self.is_unchanged(scan) {
            return Vec::new();
        }
        scan.records
            .iter()
            .filter(|r| self.uploaded.get(&r.key()).is_none_or(|&c| r.count > c))
            .cloned()
            .collect()
    }

    /// Record one accepted message's records (call per accepted batch).
    pub fn mark_uploaded(&mut self, records: &[AddonErrorRecord]) {
        for r in records {
            self.uploaded.insert(r.key(), r.count);
        }
    }

    /// Every batch of `scan` was accepted: remember the file and forget keys
    /// BugGrabber no longer holds, so the state stays bounded.
    pub fn finish_scan(&mut self, scan: &BugGrabberScan) {
        let live: Vec<String> = scan.records.iter().map(AddonErrorRecord::key).collect();
        self.uploaded.retain(|k, _| live.contains(k));
        self.file_sha256 = Some(scan.file_sha256.clone());
    }
}
