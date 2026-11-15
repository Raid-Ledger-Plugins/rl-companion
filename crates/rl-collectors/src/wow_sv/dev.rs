//! `LedgerLinkDB.dev.snapshots` (ROK-2072 §6): with `/rl dev on` the addon
//! stores one diagnostic snapshot per character as a JSON string, keyed by
//! the character's GUID exactly like `LedgerLinkDB.outbox`. This module only
//! pulls the strings out; validating the JSON is the companion's job.
//! `LedgerLinkDB.dev.probes` (ROK-2191) holds probe results the same way,
//! keyed by probe id.

use super::lua::{LuaKey, LuaValue, parse_assignments};
use super::{LEDGERLINK_DB, MAX_SV_BYTES, SkipReason, SkippedEntry, key_text};
use crate::error::WowSvError;
use std::collections::BTreeMap;

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

/// `DEV_BRIDGE_PROBE_RESULT_MAX_BYTES`: the largest probe-result string accepted.
pub const DEV_PROBE_RESULT_MAX_BYTES: usize = 65_536;

/// The usable and dropped entries of one file's `dev.probes`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DevProbes {
    /// Probe id → the result JSON exactly as the addon wrote it (unvalidated).
    pub results: BTreeMap<String, String>,
    /// Entries dropped for shape errors.
    pub skipped: Vec<SkippedEntry>,
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
    let mut out = DevSnapshots::default();
    let found = with_dev_table(bytes, "snapshots", "LedgerLinkDB.dev.snapshots", |fields| {
        for (key, value) in fields {
            match read_entry(key, value) {
                Ok(e) => out.entries.push(e),
                Err(s) => out.skipped.push(s),
            }
        }
    })?;
    if found {
        out.entries.reverse();
        out.entries.sort_by(|a, b| a.guid.cmp(&b.guid));
        out.entries
            .dedup_by(|later, earlier| later.guid == earlier.guid);
    }
    Ok(out)
}

/// Parse `LedgerLink.lua` and return `LedgerLinkDB.dev.probes` (ROK-2191):
/// probe id → the probe-result JSON exactly as the addon wrote it.
///
/// Same absent/error rules as [`parse_ledgerlink_dev_snapshots`]. A key that
/// is not a probe id (ULID), a non-string value or a string over
/// [`DEV_PROBE_RESULT_MAX_BYTES`] is skipped; the last duplicate key wins.
pub fn parse_ledgerlink_dev_probes(bytes: &[u8]) -> Result<DevProbes, WowSvError> {
    let mut out = DevProbes::default();
    with_dev_table(bytes, "probes", "LedgerLinkDB.dev.probes", |fields| {
        for (key, value) in fields {
            match read_probe(key, value) {
                Ok((id, json)) => {
                    out.results.insert(id, json);
                }
                Err(s) => out.skipped.push(s),
            }
        }
    })?;
    Ok(out)
}

/// Run `each` over the fields of `LedgerLinkDB.dev.<field>`; `Ok(false)` when
/// the file, `LedgerLinkDB`, `dev` or the field is absent.
fn with_dev_table(
    bytes: &[u8],
    field: &str,
    what: &'static str,
    each: impl FnOnce(&[(LuaKey, LuaValue)]),
) -> Result<bool, WowSvError> {
    if bytes.len() > MAX_SV_BYTES {
        return Err(WowSvError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_SV_BYTES as u64,
        });
    }
    let statements = parse_assignments(bytes)?;
    let Some((_, db)) = statements.iter().rev().find(|(n, _)| n == LEDGERLINK_DB) else {
        return Ok(false);
    };
    if !matches!(db, LuaValue::Table(_)) {
        return Err(WowSvError::NotATable {
            what: "LedgerLinkDB",
            found: db.type_name(),
        });
    }
    let Some(dev) = table_or_none(db.get("dev"), "LedgerLinkDB.dev")? else {
        return Ok(false);
    };
    let Some(LuaValue::Table(fields)) = table_or_none(dev.get(field), what)? else {
        return Ok(false);
    };
    each(fields);
    Ok(true)
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

fn read_probe(key: &LuaKey, value: &LuaValue) -> Result<(String, String), SkippedEntry> {
    let skip = |reason| SkippedEntry {
        key: key_text(key),
        reason,
    };
    let LuaKey::Str(id) = key else {
        return Err(skip(SkipReason::NonStringKey));
    };
    if !is_probe_id(id) {
        return Err(skip(SkipReason::BadProbeId));
    }
    let LuaValue::Str(json) = value else {
        return Err(skip(SkipReason::NotAString));
    };
    if json.len() > DEV_PROBE_RESULT_MAX_BYTES {
        return Err(skip(SkipReason::ProbeResultTooLarge { size: json.len() }));
    }
    Ok((id.clone(), json.clone()))
}

/// `^[0-9A-HJKMNP-TV-Z]{26}$` — a ULID (Crockford base32, no I/L/O/U).
pub fn is_probe_id(s: &str) -> bool {
    s.len() == 26
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_uppercase() && !b"ILOU".contains(&b)))
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
