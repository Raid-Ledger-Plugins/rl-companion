//! `LedgerLinkDB.dev.snapshots` (ROK-2072 §6): with `/rl dev on` the addon
//! stores one diagnostic snapshot per character as a JSON string, keyed by
//! the character's GUID exactly like `LedgerLinkDB.outbox`. This module only
//! pulls the strings out; validating the JSON is the companion's job.

use super::lua::{LuaKey, LuaValue, parse_assignments};
use super::{LEDGERLINK_DB, MAX_SV_BYTES, SkipReason, SkippedEntry, key_text};
use crate::error::WowSvError;

/// `DEV_BRIDGE_SNAPSHOT_MAX_BYTES`: the largest snapshot string accepted.
pub const DEV_SNAPSHOT_MAX_BYTES: usize = 49_152;

/// One stored snapshot: the character's GUID and the raw JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevSnapshotEntry {
    /// The table key (`Player-<realm>-<8 hex>`). Never leaves the companion.
    pub guid: String,
    /// The snapshot JSON exactly as the addon wrote it.
    pub json: String,
}

/// The usable and dropped entries of one file's `dev.snapshots`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DevSnapshots {
    /// Valid entries, sorted by GUID (the last duplicate key wins).
    pub entries: Vec<DevSnapshotEntry>,
    /// Entries dropped for shape errors.
    pub skipped: Vec<SkippedEntry>,
}

/// Parse `LedgerLink.lua` and return `LedgerLinkDB.dev.snapshots`.
///
/// No `LedgerLinkDB`, no `dev` or no `snapshots` yields an empty result (dev
/// mode off). A non-table `LedgerLinkDB` / `dev` / `snapshots` is an error.
pub fn parse_ledgerlink_dev_snapshots(bytes: &[u8]) -> Result<DevSnapshots, WowSvError> {
    if bytes.len() > MAX_SV_BYTES {
        return Err(WowSvError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_SV_BYTES as u64,
        });
    }
    let statements = parse_assignments(bytes)?;
    let Some((_, db)) = statements.iter().rev().find(|(n, _)| n == LEDGERLINK_DB) else {
        return Ok(DevSnapshots::default());
    };
    if !matches!(db, LuaValue::Table(_)) {
        return Err(WowSvError::NotATable {
            what: "LedgerLinkDB",
            found: db.type_name(),
        });
    }
    let Some(dev) = table_or_none(db.get("dev"), "LedgerLinkDB.dev")? else {
        return Ok(DevSnapshots::default());
    };
    let Some(LuaValue::Table(fields)) =
        table_or_none(dev.get("snapshots"), "LedgerLinkDB.dev.snapshots")?
    else {
        return Ok(DevSnapshots::default());
    };
    let mut out = DevSnapshots::default();
    for (key, value) in fields {
        match read_entry(key, value) {
            Ok(e) => out.entries.push(e),
            Err(s) => out.skipped.push(s),
        }
    }
    out.entries.reverse();
    out.entries.sort_by(|a, b| a.guid.cmp(&b.guid));
    out.entries
        .dedup_by(|later, earlier| later.guid == earlier.guid);
    Ok(out)
}

/// `Some(table)`, `None` for absent/nil, or a typed error for anything else.
fn table_or_none<'a>(
    v: Option<&'a LuaValue>,
    what: &'static str,
) -> Result<Option<&'a LuaValue>, WowSvError> {
    match v {
        None | Some(LuaValue::Nil) => Ok(None),
        Some(t @ LuaValue::Table(_)) => Ok(Some(t)),
        Some(other) => Err(WowSvError::NotATable {
            what,
            found: other.type_name(),
        }),
    }
}

fn read_entry(key: &LuaKey, value: &LuaValue) -> Result<DevSnapshotEntry, SkippedEntry> {
    let skip = |reason| SkippedEntry {
        key: key_text(key),
        reason,
    };
    let LuaKey::Str(guid) = key else {
        return Err(skip(SkipReason::NonStringKey));
    };
    if !is_player_guid(guid) {
        return Err(skip(SkipReason::BadGuid));
    }
    let LuaValue::Str(json) = value else {
        return Err(skip(SkipReason::NotAString));
    };
    if json.len() > DEV_SNAPSHOT_MAX_BYTES {
        return Err(skip(SkipReason::SnapshotTooLarge { size: json.len() }));
    }
    Ok(DevSnapshotEntry {
        guid: guid.clone(),
        json: json.clone(),
    })
}

/// `^Player-\d{1,5}-[0-9A-F]{8}$`
pub fn is_player_guid(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("Player-") else {
        return false;
    };
    let Some((realm, id)) = rest.split_once('-') else {
        return false;
    };
    (1..=5).contains(&realm.len())
        && realm.bytes().all(|b| b.is_ascii_digit())
        && id.len() == 8
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}
