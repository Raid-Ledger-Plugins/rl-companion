//! LedgerLink SavedVariables collector (ROK-1964 §5).
//!
//! The LedgerLink addon stores its latest export run per character in
//! `LedgerLinkDB.outbox[guid] = { exportedAt, section, text }`; the WoW
//! client writes that to `WTF/Account/<ACCOUNT>/SavedVariables/LedgerLink.lua`
//! on logout, `/reload` and clean exit. [`parse_ledgerlink_outbox`] reads the
//! entries out of that file's bytes, [`changed_entries`] picks the ones whose
//! text changed since the last upload, and [`ledgerlink_sv_candidates`] finds
//! the files in a directory listing. No filesystem access, no Lua
//! interpreter: [`lua`] parses only the subset the client writes.

mod lua;
mod paths;

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::WowSvError;
use crate::extract::sha256_hex;
use lua::{LuaKey, LuaValue, parse_assignments};

pub use paths::{
    LEDGERLINK_SV_FILE, SvCandidate, is_flavor_folder, ledgerlink_sv_candidates, ledgerlink_sv_path,
};

/// The account-wide SavedVariables table the addon declares.
pub const LEDGERLINK_DB: &str = "LedgerLinkDB";
/// Largest `LedgerLink.lua` the parser accepts.
pub const MAX_SV_BYTES: usize = 16 * 1024 * 1024;
/// Largest export text accepted (`ADDON_IMPORT_MAX_BYTES` on the server).
pub const MAX_EXPORT_TEXT_BYTES: usize = 262_144;

/// One outbox entry: the latest export run of one character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxEntry {
    /// The outbox key: the exporting character's GUID (`Player-…`).
    pub guid: String,
    /// `exportedAt` (unix seconds), when present and an integer.
    pub exported_at: Option<i64>,
    /// `section` (`char` | `guild` | `raid` | `all`), when present.
    pub section: Option<String>,
    /// The page tokens exactly as the copy window shows them.
    pub text: String,
    /// Lower-case hex SHA-256 of `text`, for dedupe.
    pub sha256: String,
}

/// Why an outbox entry was not returned.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// The key is not a string (e.g. a positional entry).
    NonStringKey,
    /// The value is not a table.
    NotATable,
    /// No `text` field, or it is not a string.
    MissingText,
    /// `text` holds no tokens.
    EmptyText,
    /// `text` is over [`MAX_EXPORT_TEXT_BYTES`].
    TextTooLarge { size: usize },
    /// Token `index` (0-based) does not match `ADDON_IMPORT_PAGE_RE`.
    InvalidToken { index: usize },
}

/// An outbox entry that was dropped (log it locally; never upload it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    /// The outbox key as text (`[1]` for a positional entry).
    pub key: String,
    pub reason: SkipReason,
}

/// The usable and dropped entries of one file's outbox.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outbox {
    /// Valid entries, sorted by GUID.
    pub entries: Vec<OutboxEntry>,
    /// Entries dropped for shape or token errors.
    pub skipped: Vec<SkippedEntry>,
}

/// Parse `LedgerLink.lua` and return `LedgerLinkDB.outbox`.
///
/// A file without `LedgerLinkDB`, or whose `LedgerLinkDB` has no `outbox`,
/// yields an empty [`Outbox`] (nothing to upload). Syntax outside the
/// supported subset, or a non-table `LedgerLinkDB` / `outbox`, is an error.
pub fn parse_ledgerlink_outbox(bytes: &[u8]) -> Result<Outbox, WowSvError> {
    if bytes.len() > MAX_SV_BYTES {
        return Err(WowSvError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_SV_BYTES as u64,
        });
    }
    let statements = parse_assignments(bytes)?;
    let Some((_, db)) = statements.iter().rev().find(|(n, _)| n == LEDGERLINK_DB) else {
        return Ok(Outbox::default());
    };
    let LuaValue::Table(_) = db else {
        return Err(WowSvError::NotATable {
            what: "LedgerLinkDB",
            found: db.type_name(),
        });
    };
    let fields = match db.get("outbox") {
        None | Some(LuaValue::Nil) => return Ok(Outbox::default()),
        Some(LuaValue::Table(fields)) => fields,
        Some(other) => {
            return Err(WowSvError::NotATable {
                what: "LedgerLinkDB.outbox",
                found: other.type_name(),
            });
        }
    };
    let mut outbox = Outbox::default();
    for (key, value) in fields {
        match read_entry(key, value) {
            Ok(entry) => outbox.entries.push(entry),
            Err(skipped) => outbox.skipped.push(skipped),
        }
    }
    // Stable sort after a reverse: the last duplicate key wins, as in Lua.
    outbox.entries.reverse();
    outbox.entries.sort_by(|a, b| a.guid.cmp(&b.guid));
    outbox
        .entries
        .dedup_by(|later, earlier| later.guid == earlier.guid);
    Ok(outbox)
}

fn read_entry(key: &LuaKey, value: &LuaValue) -> Result<OutboxEntry, SkippedEntry> {
    let skip = |reason| SkippedEntry {
        key: key_text(key),
        reason,
    };
    let LuaKey::Str(guid) = key else {
        return Err(skip(SkipReason::NonStringKey));
    };
    let LuaValue::Table(_) = value else {
        return Err(skip(SkipReason::NotATable));
    };
    let Some(LuaValue::Str(text)) = value.get("text") else {
        return Err(skip(SkipReason::MissingText));
    };
    validate_export_text(text).map_err(skip)?;
    let exported_at = match value.get("exportedAt") {
        Some(LuaValue::Num(n)) => whole_seconds(*n),
        _ => None,
    };
    let section = match value.get("section") {
        Some(LuaValue::Str(s)) => Some(s.clone()),
        _ => None,
    };
    Ok(OutboxEntry {
        guid: guid.clone(),
        exported_at,
        section,
        text: text.clone(),
        sha256: sha256_hex(text.as_bytes()),
    })
}

fn whole_seconds(n: f64) -> Option<i64> {
    // 2^53: every integer below it is exact in an f64.
    const LIMIT: f64 = 9_007_199_254_740_992.0;
    (n.fract() == 0.0 && n.abs() < LIMIT).then_some(n as i64)
}

fn key_text(key: &LuaKey) -> String {
    match key {
        LuaKey::Str(s) => s.clone(),
        LuaKey::Num(n) => format!("[{n}]"),
        LuaKey::Bool(b) => format!("[{b}]"),
    }
}

/// Check an export text: 1+ whitespace-separated tokens, each matching
/// `ADDON_IMPORT_PAGE_RE`, at most [`MAX_EXPORT_TEXT_BYTES`] in all.
pub fn validate_export_text(text: &str) -> Result<(), SkipReason> {
    if text.len() > MAX_EXPORT_TEXT_BYTES {
        return Err(SkipReason::TextTooLarge { size: text.len() });
    }
    let mut count = 0;
    for (index, token) in text.split_ascii_whitespace().enumerate() {
        if !is_valid_page_token(token) {
            return Err(SkipReason::InvalidToken { index });
        }
        count += 1;
    }
    if count == 0 {
        return Err(SkipReason::EmptyText);
    }
    Ok(())
}

/// `^!RL(\d+)!(char|guild|raid)(?:-(\d+)of(\d+))?!([A-Za-z0-9+/]+={0,2})$`
pub fn is_valid_page_token(token: &str) -> bool {
    fn digits(s: &str) -> (&str, &str) {
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        s.split_at(end)
    }
    let Some(rest) = token.strip_prefix("!RL") else {
        return false;
    };
    let (version, rest) = digits(rest);
    let Some(rest) = rest.strip_prefix('!').filter(|_| !version.is_empty()) else {
        return false;
    };
    let Some(rest) = ["char", "guild", "raid"]
        .iter()
        .find_map(|section| rest.strip_prefix(section))
    else {
        return false;
    };
    let rest = match rest.strip_prefix('-') {
        None => rest,
        Some(paged) => {
            let (n, after) = digits(paged);
            let Some(after) = after.strip_prefix("of").filter(|_| !n.is_empty()) else {
                return false;
            };
            let (m, after) = digits(after);
            if m.is_empty() {
                return false;
            }
            after
        }
    };
    let Some(body) = rest.strip_prefix('!') else {
        return false;
    };
    let data = body.trim_end_matches('=');
    let padding = body.len() - data.len();
    padding <= 2
        && !data.is_empty()
        && data
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// The entries whose text changed since the last accepted upload.
///
/// `previous` maps GUID → the SHA-256 last accepted for that GUID in this
/// account folder (the caller keys its state by `(account folder, guid)`).
pub fn changed_entries<'a>(
    entries: &'a [OutboxEntry],
    previous: &BTreeMap<String, String>,
) -> Vec<&'a OutboxEntry> {
    entries
        .iter()
        .filter(|e| previous.get(&e.guid) != Some(&e.sha256))
        .collect()
}
