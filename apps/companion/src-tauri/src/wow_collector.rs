//! The WoW LedgerLink collector (ROK-1964 §5): reads every account-wide
//! `LedgerLink.lua` under the WoW folder and turns each changed outbox entry
//! into one `wow-ledgerlink/v1` message.

use crate::collector::{envelope, scope_key, Collector, PendingUpload, Scan, SentState};
use crate::scope::WatchScope;
use rl_collectors::{
    changed_entries, is_flavor_folder, ledgerlink_sv_candidates, parse_ledgerlink_outbox,
    OutboxEntry, SvCandidate, LEDGERLINK_SV_FILE, MAX_SV_BYTES,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const CONTRACT: &str = "wow-ledgerlink";
pub const CONTRACT_VERSION: u32 = 1;
/// `COMPANION_WOW_LEDGERLINK_EXPORT_MAX` (contract §3) — 2 KiB under the
/// paste limit so the envelope fits the 256 KiB body ceiling.
pub const EXPORT_MAX: usize = 260_096;

pub struct LedgerLinkCollector {
    root: PathBuf,
    /// Every read goes through this (ROK-2061): nothing outside the WoW folder.
    scope: WatchScope,
}

impl LedgerLinkCollector {
    /// `root` is the WoW install folder (the one holding `_classic_era_` etc.);
    /// reads are confined to it.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let scope = WatchScope::new(std::slice::from_ref(&root));
        Self::scoped(root, scope)
    }

    /// As [`Self::new`], with the scope the registry resolved.
    pub fn scoped(root: impl Into<PathBuf>, scope: WatchScope) -> Self {
        Self {
            root: root.into(),
            scope,
        }
    }

    /// Every `<root>/<flavor>/WTF` folder that exists inside the scope.
    fn wtf_dirs(&self) -> Vec<PathBuf> {
        self.scope
            .read_dir(&self.root)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_flavor_folder)
            })
            .map(|p| p.join("WTF"))
            .filter(|p| self.scope.is_dir(p))
            .collect()
    }

    /// Files under `<flavor>/WTF/Account/<account>/SavedVariables/`, relative
    /// to the root — only the depth the account-wide file lives at, so the
    /// (huge) rest of the install is never walked.
    pub fn listing(&self) -> Vec<String> {
        let mut out = Vec::new();
        for wtf in self.wtf_dirs() {
            for account in self.read_dirs(&wtf.join("Account")) {
                for file in self.read_files(&account.join("SavedVariables")) {
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

    fn scan_file(&self, cand: &SvCandidate, origin: &str, sent: &SentState, scan: &mut Scan) {
        let path = Path::new(&cand.path);
        let meta = match self.scope.metadata(path) {
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
        let outbox = match self.scope.read(path).map(|b| parse_ledgerlink_outbox(&b)) {
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
        let scope = scope_key(origin, &cand.path);
        let previous = sent.scope(&scope);
        for entry in changed_entries(&outbox.entries, &previous) {
            if entry.text.len() > EXPORT_MAX {
                scan.notes.push(format!(
                    "{}: export for {} is over the upload cap; paste it instead",
                    cand.account, entry.guid
                ));
                continue;
            }
            scan.pending.push(PendingUpload {
                campaign: None,
                contract: CONTRACT,
                scope: scope.clone(),
                item: entry.guid.clone(),
                sha256: entry.sha256.clone(),
                label: label(entry),
                body: message(&entry.text, &captured_at),
                blobs: Vec::new(),
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

    fn scan(&self, origin: &str, sent: &SentState) -> Scan {
        let mut scan = Scan::default();
        for cand in self.candidates() {
            self.scan_file(&cand, origin, sent, &mut scan);
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
    crate::collector::captured_at(mtime)
}

fn label(entry: &OutboxEntry) -> String {
    format!(
        "{} ({})",
        entry.guid,
        entry.section.as_deref().unwrap_or("export")
    )
}

impl LedgerLinkCollector {
    fn read_dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = self.scope.read_dir(dir).unwrap_or_default();
        out.retain(|p| self.scope.is_dir(p));
        out
    }

    fn read_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = self.scope.read_dir(dir).unwrap_or_default();
        out.retain(|p| self.scope.is_file(p));
        out
    }
}
