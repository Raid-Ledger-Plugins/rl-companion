//! What `/dev/v1/collectors` reports beyond the window's rows: when each
//! collector last scanned, how many items that scan left unsettled, and when
//! the server last accepted an upload from it. The accepted-upload time is
//! persisted next to `sent-state.json`, so a restart does not blank it.
//!
//! Also the bridge-safe form of an Activity line ([`scrub_paths`]), mirrored
//! into the dev-bridge log ring: no subject (a character, account, save or
//! campaign name — callers pass it apart) and no absolute path.

use crate::collector::{PendingUpload, Scan, SentState};
use crate::registry::Entry;
use crate::runtime::Runtime;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// File next to `sent-state.json`: `collector id → last accepted upload`.
pub const UPLOAD_TIMES_FILE: &str = "upload-times.json";

/// One collector's line in `/dev/v1/collectors` (beyond `enabled`/`paused`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub last_scan_at: Option<String>,
    pub pending: usize,
    pub last_upload_at: Option<String>,
}

/// The persisted accepted-upload times (RFC 3339 UTC per collector id).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadTimes(pub BTreeMap<String, String>);

impl UploadTimes {
    /// Missing or unreadable file (an install that predates it) → empty.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write via a temp file + rename, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    }
}

/// Where [`UploadTimes`] lives: beside the sent-state file.
pub fn upload_times_path(sent_path: &Path) -> PathBuf {
    sent_path.with_file_name(UPLOAD_TIMES_FILE)
}

/// Per-collector scan facts plus the persisted upload times.
#[derive(Debug, Default, Clone)]
pub struct ScanBook {
    scans: BTreeMap<String, (String, usize)>,
    pub uploads: UploadTimes,
}

impl ScanBook {
    pub fn new(uploads: UploadTimes) -> Self {
        Self {
            scans: BTreeMap::new(),
            uploads,
        }
    }

    /// A scan of `id` ran at `at` and left `pending` items unsettled.
    pub fn scanned(&mut self, id: &str, at: String, pending: usize) {
        self.scans.insert(id.to_owned(), (at, pending));
    }

    /// `id` is switched off: it did not scan, so it reports no scan.
    pub fn forget_scan(&mut self, id: &str) {
        self.scans.remove(id);
    }

    /// The server accepted an upload from `id` at `at` (kept if later).
    pub fn uploaded(&mut self, id: &str, at: String) {
        let slot = self.uploads.0.entry(id.to_owned()).or_default();
        if at > *slot {
            *slot = at;
        }
    }

    pub fn reports(&self) -> BTreeMap<String, ScanReport> {
        let ids = self.scans.keys().chain(self.uploads.0.keys());
        ids.map(|id| {
            let scan = self.scans.get(id);
            let report = ScanReport {
                last_scan_at: scan.map(|(at, _)| at.clone()),
                pending: scan.map_or(0, |(_, n)| *n),
                last_upload_at: self.uploads.0.get(id).cloned(),
            };
            (id.clone(), report)
        })
        .collect()
    }
}

/// One enabled collector's pass over a fresh scan. Dropping it stamps the
/// scan ([`Runtime::scanned`]) with what the pass left unsettled, so no return
/// path of the upload can skip the stamp.
pub struct ScanPass<'a> {
    rt: &'a Runtime,
    id: &'static str,
    pub scan: Scan,
}

impl<'a> ScanPass<'a> {
    /// Read `entry`; `None` (and its last scan forgotten) when it is off.
    pub fn begin(rt: &'a Runtime, entry: &Entry, origin: &str) -> Option<Self> {
        let Some(scan) = entry.scan(origin, &rt.sent()) else {
            rt.forget_scan(entry.manifest.id);
            return None;
        };
        Some(Self {
            rt,
            id: entry.manifest.id,
            scan,
        })
    }
}

impl Drop for ScanPass<'_> {
    fn drop(&mut self) {
        self.rt
            .scanned(self.id, &self.scan.pending, self.scan.waiting);
    }
}

/// How many of a scan's items are still unsettled in `sent` (neither
/// accepted nor rejected terminally): what is queued for a later pass.
pub fn unsettled(sent: &SentState, pending: &[PendingUpload]) -> usize {
    pending
        .iter()
        .filter(|p| sent.0.get(&p.scope).and_then(|m| m.get(&p.item)) != Some(&p.sha256))
        .count()
}

/// Characters after which a path may start (a token boundary).
const BOUNDARY: &str = "\"'[(<{|:=,;";

/// The closing partner of an opening quote or bracket.
fn closer_of(open: char) -> Option<char> {
    Some(match open {
        '"' => '"',
        '\'' => '\'',
        '[' => ']',
        '(' => ')',
        '<' => '>',
        '{' => '}',
        _ => return None,
    })
}

fn at_token_start(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(p) => p.is_whitespace() || BOUNDARY.contains(p),
    }
}

/// Does an absolute path start here? A drive (`C:\`, `C:/`), UNC, verbatim
/// or rooted (`\\host`, `\\?\`, `\Users`), home (`~/`), `file:/`, or a
/// rooted Unix path (`/x`, but not `/ ` as in `5 / 10`, nor `//` in a URL).
fn path_starts(rest: &str) -> bool {
    let b = rest.as_bytes();
    let sep = |i: usize| matches!(b.get(i), Some(b'\\' | b'/'));
    match b.first() {
        Some(c) if c.is_ascii_alphabetic() && b.get(1) == Some(&b':') && sep(2) => true,
        Some(b'\\') => b.get(1).is_some_and(|c| !c.is_ascii_whitespace()),
        Some(b'~') => sep(1),
        Some(b'/') => b
            .get(1)
            .is_some_and(|c| !c.is_ascii_whitespace() && !b"\\/\"'()[]{},;<>|".contains(c)),
        _ => rest
            .get(..6)
            .is_some_and(|s| s.eq_ignore_ascii_case("file:/")),
    }
}

/// Where a path opened by a quote or bracket ends: the LAST `close` on the
/// line that a token break follows. A path may hold the closer itself
/// (`O'Brien`, `(x86)`), so the last one is the only safe end.
fn closing(line: &str, from: usize, close: char) -> Option<usize> {
    let ends = |j: usize| match line[j..].chars().nth(1) {
        None => true,
        Some(c) => c.is_whitespace() || ")]}>,;:\"'|!?".contains(c),
    };
    line[from..]
        .char_indices()
        .map(|(j, c)| (from + j, c))
        .filter(|&(j, c)| c == close && ends(j))
        .next_back()
        .map(|(j, _)| j)
}

fn scrub_line(line: &str, out: &mut String) {
    let mut prev = None;
    let mut i = 0;
    while let Some(c) = line[i..].chars().next() {
        if at_token_start(prev) && path_starts(&line[i..]) {
            out.push_str("<path>");
            match prev.and_then(closer_of).and_then(|cl| closing(line, i, cl)) {
                // Resume at the closer; `>` stands for the `<path>` just written.
                Some(j) => (i, prev) = (j, Some('>')),
                // Bare, or never closed: the rest of the line goes.
                None => return,
            }
            continue;
        }
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
}

/// `text` as the dev bridge may store it, once the caller has left the
/// subject out. Fail-closed: from where an absolute path starts, the rest of
/// the line is `<path>`, unless the path opened right after a quote or
/// bracket, which then ends it (see [`closing`]) and scrubbing goes on after.
/// Over-redaction is the accepted cost. The ring then applies its own
/// redaction (tokens, GUIDs, homes).
pub fn scrub_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in text.split_inclusive('\n') {
        let line = piece.trim_end_matches(['\r', '\n']);
        scrub_line(line, &mut out);
        out.push_str(&piece[line.len()..]);
    }
    out
}

/// The ring's `target` for an Activity line.
pub fn bridge_target(collector_id: Option<&str>) -> String {
    match collector_id {
        Some(id) => format!("collector:{id}"),
        None => "collector".to_owned(),
    }
}
