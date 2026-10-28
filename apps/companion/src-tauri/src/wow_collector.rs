//! The WoW LedgerLink collector (ROK-1964 §5): reads every account-wide
//! `LedgerLink.lua` under the WoW folder and turns each changed outbox entry
//! into one `wow-ledgerlink/v1` message.

use crate::collector::{envelope, Collector, PendingUpload, Scan, SentState};
use rl_collectors::{
    changed_entries, is_flavor_folder, ledgerlink_sv_candidates, parse_ledgerlink_outbox,
    rfc3339_utc_from_unix_seconds, OutboxEntry, SvCandidate, LEDGERLINK_SV_FILE, MAX_SV_BYTES,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONTRACT: &str = "wow-ledgerlink";
pub const CONTRACT_VERSION: u32 = 1;
/// `COMPANION_WOW_LEDGERLINK_EXPORT_MAX` (contract §3) — 2 KiB under the
/// paste limit so the envelope fits the 256 KiB body ceiling.
pub const EXPORT_MAX: usize = 260_096;

pub struct LedgerLinkCollector {
    root: PathBuf,
}

impl LedgerLinkCollector {
    /// `root` is the WoW install folder (the one holding `_classic_era_` etc.).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Every `<root>/<flavor>/WTF` folder that exists.
    fn wtf_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(is_flavor_folder))
            .map(|e| e.path().join("WTF"))
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs
    }

    /// Files under `<flavor>/WTF/Account/<account>/SavedVariables/`, relative
    /// to the root — only the depth the account-wide file lives at, so the
    /// (huge) rest of the install is never walked.
    pub fn listing(&self) -> Vec<String> {
        let mut out = Vec::new();
        for wtf in self.wtf_dirs() {
            for account in read_dirs(&wtf.join("Account")) {
                for file in read_files(&account.join("SavedVariables")) {
                    if let Ok(rel) = file.strip_prefix(&self.root) {
                        out.push(rel.to_string_lossy().into_owned());
                    }
                }
            }
        }
        out
    }

    pub fn candidates(&self) -> Vec<SvCandidate> {
        let root = self.root.to_string_lossy().into_owned();
        let listing = self.listing();
        ledgerlink_sv_candidates(&root, listing.iter().map(String::as_str))
    }

    fn scan_file(&self, cand: &SvCandidate, sent: &SentState, scan: &mut Scan) {
        let path = Path::new(&cand.path);
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                return scan.notes.push(format!(
                    "{}: cannot read LedgerLink.lua ({e})",
                    cand.account
                ))
            }
        };
        if meta.len() > MAX_SV_BYTES as u64 {
            return scan.notes.push(format!(
                "{}: LedgerLink.lua is over 16 MiB; skipped",
                cand.account
            ));
        }
        let outbox = match fs::read(path).map(|b| parse_ledgerlink_outbox(&b)) {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return scan.notes.push(format!(
                    "{}: LedgerLink.lua not readable yet ({e})",
                    cand.account
                ))
            }
            Err(e) => {
                return scan.notes.push(format!(
                    "{}: cannot read LedgerLink.lua ({e})",
                    cand.account
                ))
            }
        };
        scan.files += 1;
        for s in &outbox.skipped {
            scan.notes.push(format!(
                "{}: skipped outbox entry {} ({:?})",
                cand.account, s.key, s.reason
            ));
        }
        let captured_at = captured_at(meta.modified().ok());
        let previous = sent.scope(&cand.path);
        for entry in changed_entries(&outbox.entries, &previous) {
            if entry.text.len() > EXPORT_MAX {
                scan.notes.push(format!(
                    "{}: export for {} is over the upload cap; paste it instead",
                    cand.account, entry.guid
                ));
                continue;
            }
            scan.pending.push(PendingUpload {
                contract: CONTRACT,
                scope: cand.path.clone(),
                item: entry.guid.clone(),
                sha256: entry.sha256.clone(),
                label: label(entry),
                body: message(&entry.text, &captured_at),
            });
        }
    }
}

impl Collector for LedgerLinkCollector {
    fn id(&self) -> &'static str {
        CONTRACT
    }

    fn watch_roots(&self) -> Vec<PathBuf> {
        self.wtf_dirs()
    }

    fn is_relevant(&self, path: &Path) -> bool {
        is_ledgerlink_file(path)
    }

    fn scan(&self, sent: &SentState) -> Scan {
        let mut scan = Scan::default();
        for cand in self.candidates() {
            self.scan_file(&cand, sent, &mut scan);
        }
        scan
    }
}

/// `LedgerLink.lua`, any case (Windows folders are case-insensitive).
pub fn is_ledgerlink_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(LEDGERLINK_SV_FILE))
}

/// One `wow-ledgerlink/v1` message: the export text verbatim (contract §2).
pub fn message(export_string: &str, captured_at: &str) -> serde_json::Value {
    envelope(
        CONTRACT,
        CONTRACT_VERSION,
        captured_at,
        json!({ "exportString": export_string }),
    )
}

/// `capturedAt` = the file's mtime (spec §5), never in the future (the
/// server rejects more than 5 minutes ahead of its clock).
pub fn captured_at(mtime: Option<SystemTime>) -> String {
    let now = SystemTime::now();
    let t = mtime.filter(|m| *m <= now).unwrap_or(now);
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339_utc_from_unix_seconds(i64::try_from(secs).unwrap_or(0))
}

fn label(entry: &OutboxEntry) -> String {
    format!(
        "{} ({})",
        entry.guid,
        entry.section.as_deref().unwrap_or("export")
    )
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn read_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default()
}
