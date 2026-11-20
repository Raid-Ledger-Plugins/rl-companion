//! The WoW addon-errors collector (ROK-2099 S5, contract `wow-addon-errors/v1`):
//! reads every account-wide `!BugGrabber.lua` under the WoW folder, scrubs and
//! merges its errors (the `rl-collectors` core) and turns the records the
//! server has not accepted yet into messages of ≤ 50 records / 256 KiB.
//!
//! Off by default: a disabled registry entry never calls [`Collector::scan`],
//! so nothing is listed or read (AC4). Dedupe lives in the shared
//! [`SentState`]: one item per record (`record key → accepted count`), written
//! by the runtime only once the server settled the message
//! ([`crate::collector::settled_items`]) — a failed upload is re-sent on the
//! next scan.

use crate::addon_errors_key;
use crate::collector::{scope_key, Collector, PendingUpload, Scan, SentState};
use crate::scope::WatchScope;
use crate::wow_collector::LedgerLinkCollector;
use rl_collectors::{
    account_characters, account_sv_candidates, build_messages, scan_buggrabber, sha256_hex,
    toc_version, AddonErrorRecord, AddonErrorsMessage, AddonErrorsState, PayloadHeader,
    ScrubContext, SvCandidate, TimeContext, ADDON_ERRORS_CONTRACT, ADDON_ERRORS_CONTRACT_VERSION,
    BUGGRABBER_SV_FILE, MAX_SV_BYTES,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const CONTRACT: &str = ADDON_ERRORS_CONTRACT;
pub const CONTRACT_VERSION: u32 = ADDON_ERRORS_CONTRACT_VERSION;
/// `sourceVersion` when BugGrabber's `.toc` is missing or unreadable.
pub const UNKNOWN_SOURCE_VERSION: &str = "unknown";

pub struct AddonErrorsCollector {
    root: PathBuf,
    /// Every read goes through this: nothing outside the WoW folder.
    scope: WatchScope,
    /// Shares LedgerLink's WTF walk (same depth, same scope).
    files: LedgerLinkCollector,
    /// Local = UTC + offset; BugGrabber writes local wall-clock times.
    utc_offset_seconds: i32,
    /// Where the `accountKey` install secret lives (default: the app data dir
    /// set at startup, see [`crate::addon_errors_key`]).
    key_dir: Option<PathBuf>,
    key: OnceLock<Option<[u8; 32]>>,
}

impl AddonErrorsCollector {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let scope = WatchScope::new(std::slice::from_ref(&root));
        Self::scoped(root, scope)
    }

    /// As [`Self::new`], with the scope the registry resolved and this
    /// machine's current UTC offset.
    pub fn scoped(root: impl Into<PathBuf>, scope: WatchScope) -> Self {
        let root = root.into();
        Self {
            files: LedgerLinkCollector::scoped(root.clone(), scope.clone()),
            root,
            scope,
            utc_offset_seconds: local_utc_offset_seconds(),
            key_dir: None,
            key: OnceLock::new(),
        }
    }

    /// Keep the `accountKey` install secret in `dir` (tests).
    pub fn with_key_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.key_dir = Some(dir.into());
        self
    }

    /// The per-install HMAC key, loaded (or created) once per collector.
    fn install_key(&self) -> Option<[u8; 32]> {
        *self.key.get_or_init(|| {
            let dir = self.key_dir.clone().or_else(addon_errors_key::default_dir);
            addon_errors_key::load_or_create(dir.as_deref())
        })
    }

    /// Pin the UTC offset (tests).
    pub fn with_utc_offset(mut self, seconds: i32) -> Self {
        self.utc_offset_seconds = seconds;
        self
    }

    pub fn candidates(&self) -> Vec<SvCandidate> {
        let root = self.root.to_string_lossy().into_owned();
        let listing = self.files.listing();
        account_sv_candidates(
            &root,
            listing.iter().map(String::as_str),
            BUGGRABBER_SV_FILE,
        )
    }

    fn scan_file(&self, cand: &SvCandidate, origin: &str, sent: &SentState, scan: &mut Scan) {
        let note = |scan: &mut Scan, what: String| {
            scan.notes
                .push(format!("{} ({}): {what}", cand.account, cand.flavor))
        };
        let path = Path::new(&cand.path);
        let meta = match self.scope.metadata(path) {
            Ok(m) => m,
            Err(e) => return note(scan, format!("cannot read !BugGrabber.lua ({e})")),
        };
        if meta.len() > MAX_SV_BYTES as u64 {
            return note(scan, "!BugGrabber.lua is over 16 MiB; skipped".into());
        }
        let bytes = match self.scope.read(path) {
            Ok(b) => b,
            Err(e) => return note(scan, format!("cannot read !BugGrabber.lua ({e})")),
        };
        let mtime = meta.modified().ok();
        let time = TimeContext {
            utc_offset_seconds: self.utc_offset_seconds,
            fallback_unix: unix_seconds(mtime),
        };
        let parsed = match scan_buggrabber(&bytes, &self.scrub_context(cand), &time) {
            Ok(p) => p,
            Err(e) => return note(scan, format!("!BugGrabber.lua not readable yet ({e})")),
        };
        scan.files += 1;
        if parsed.skipped > 0 {
            note(
                scan,
                format!("skipped {} unusable error entries", parsed.skipped),
            );
        }
        let scope = scope_key(origin, &cand.path);
        let accepted = accepted_state(sent, &scope);
        let records = accepted.pending(&parsed);
        let mut live = accepted.clone();
        live.finish_scan(&parsed);
        if live.uploaded.len() < accepted.uploaded.len() {
            scan.retain
                .push((scope.clone(), live.uploaded.into_keys().collect()));
        }
        let Some(key) = self.install_key() else {
            return note(
                scan,
                "addon errors not sent (no OS randomness for the key)".into(),
            );
        };
        let messages = PayloadHeader::new(
            &self.source_version(cand),
            &cand.account,
            &key,
            None,
            Some(&cand.flavor),
        )
        .and_then(|h| {
            let at = crate::collector::captured_at(mtime);
            build_messages(&h, records, &at, crate::app_version::get())
        });
        match messages {
            Ok(ms) => scan
                .pending
                .extend(ms.iter().map(|m| pending(m, &scope, cand))),
            Err(e) => note(scan, format!("addon errors not sent ({e})")),
        }
    }

    /// Character + realm folder names of the account, scrubbed from the text.
    fn scrub_context(&self, cand: &SvCandidate) -> ScrubContext {
        let account = self
            .root
            .join(&cand.flavor)
            .join("WTF")
            .join("Account")
            .join(&cand.account);
        let mut listing = Vec::new();
        for realm in self.dirs(&account) {
            for character in self.dirs(&realm) {
                if let Ok(rel) = character.strip_prefix(&self.root) {
                    listing.push(format!("{}/_", rel.to_string_lossy()));
                }
            }
        }
        let root = self.root.to_string_lossy().into_owned();
        let pairs = account_characters(
            &root,
            listing.iter().map(String::as_str),
            &cand.flavor,
            &cand.account,
        );
        let mut realms: Vec<String> = pairs.iter().map(|(r, _)| r.clone()).collect();
        realms.dedup();
        ScrubContext {
            characters: pairs.into_iter().map(|(_, c)| c).collect(),
            realms,
            battle_tag: None,
            account: Some(cand.account.clone()),
        }
    }

    /// `## Version:` of `<flavor>/Interface/AddOns/!BugGrabber/!BugGrabber.toc`.
    fn source_version(&self, cand: &SvCandidate) -> String {
        let toc = self
            .root
            .join(&cand.flavor)
            .join("Interface")
            .join("AddOns")
            .join("!BugGrabber")
            .join("!BugGrabber.toc");
        self.scope
            .read(&toc)
            .ok()
            .and_then(|b| toc_version(&b))
            .unwrap_or_else(|| UNKNOWN_SOURCE_VERSION.to_owned())
    }

    fn dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out = self.scope.read_dir(dir).unwrap_or_default();
        out.retain(|p| {
            self.scope.is_dir(p)
                && !p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("SavedVariables"))
        });
        out
    }
}

impl Collector for AddonErrorsCollector {
    fn id(&self) -> &'static str {
        CONTRACT
    }

    fn watch_roots(&self) -> Vec<PathBuf> {
        self.files.watch_roots()
    }

    fn scan(&self, origin: &str, sent: &SentState) -> Scan {
        let mut scan = Scan::default();
        for cand in self.candidates() {
            self.scan_file(&cand, origin, sent, &mut scan);
        }
        scan
    }
}

/// The counts the server already accepted for one file, as the core's state.
pub fn accepted_state(sent: &SentState, scope: &str) -> AddonErrorsState {
    AddonErrorsState {
        file_sha256: None,
        uploaded: sent
            .scope(scope)
            .into_iter()
            .filter_map(|(k, v)| v.parse().ok().map(|c| (k, c)))
            .collect(),
    }
}

/// One message as a pending upload. `item` names the batch for the window;
/// what is recorded on settle is [`record_items`].
fn pending(msg: &AddonErrorsMessage, scope: &str, cand: &SvCandidate) -> PendingUpload {
    let body = serde_json::to_value(msg).unwrap_or(Value::Null);
    let keys: Vec<String> = msg
        .payload
        .records
        .iter()
        .map(AddonErrorRecord::key)
        .collect();
    let batch = sha256_hex(keys.join(",").as_bytes());
    PendingUpload {
        campaign: None,
        contract: CONTRACT,
        scope: scope.to_owned(),
        item: format!("batch:{}", &batch[..16]),
        sha256: sha256_hex(body.to_string().as_bytes()),
        label: format!(
            "{} addon error(s) ({})",
            msg.payload.records.len(),
            cand.flavor
        ),
        body,
        blobs: Vec::new(),
    }
}

/// `(record key, count)` for every record of a settled message body.
pub fn record_items(body: &Value) -> Vec<(String, String)> {
    body.pointer("/payload/records")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .filter_map(|r| serde_json::from_value::<AddonErrorRecord>(r.clone()).ok())
                .map(|r| (r.key(), r.count.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A content rejection settles the batch, so it neither blocks later
/// uploads nor loops; this says how many errors that dropped, never their text.
pub fn refused_note(p: &PendingUpload, fx: &crate::runtime::Effect) -> Option<String> {
    (p.contract == CONTRACT && fx.record && !fx.ok).then(|| {
        let n = p
            .body
            .pointer("/payload/records")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        format!("{n} addon error(s) refused by the server and not re-sent")
    })
}

fn unix_seconds(t: Option<std::time::SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// This machine's current UTC offset in seconds.
pub fn local_utc_offset_seconds() -> i32 {
    chrono::Local::now().offset().local_minus_utc()
}
