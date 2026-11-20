//! `!BugGrabber.lua` → scrubbed, merged [`AddonErrorRecord`]s.

use std::collections::BTreeMap;

use super::scrub::{has_unscrubbed_path, scrub_names, scrub_paths};
use super::text::{mask_addresses, multi_line, one_line};
use super::{
    ADDON_ERROR_ADDON_MAX, ADDON_ERROR_COUNT_MAX, ADDON_ERROR_MESSAGE_MAX, ADDON_ERROR_STACK_MAX,
    AddonErrorRecord, BUGGRABBER_DB,
};
use crate::error::WowSvError;
use crate::extract::sha256_hex;
use crate::time::{days_from_civil, rfc3339_utc_from_unix_seconds};
use crate::wow_sv::MAX_SV_BYTES;
use crate::wow_sv::lua::{LuaValue, parse_assignments};

/// The account's own names, replaced in `message` / `stack` (contract §4.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrubContext {
    /// Character folder names under `WTF/Account/<acct>/<realm>/`.
    pub characters: Vec<String>,
    /// Realm folder names under `WTF/Account/<acct>/`.
    pub realms: Vec<String>,
    /// The BattleTag (`Name#1234`), when known.
    pub battle_tag: Option<String>,
    /// The `WTF/Account/<folder>` name (an old login name, or
    /// `<digits>#<n>`): never sent (contract §4.2), replaced by `<account>`.
    pub account: Option<String>,
}

impl ScrubContext {
    fn names(&self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = Vec::new();
        if let Some(account) = &self.account {
            out.push((account.clone(), "<account>"));
            if let Some((number, _)) = account.split_once('#') {
                out.push((number.to_owned(), "<account>"));
            }
        }
        for c in &self.characters {
            out.push((c.clone(), "<char>"));
        }
        for r in &self.realms {
            out.push((r.clone(), "<realm>"));
            out.push((r.split_whitespace().collect(), "<realm>"));
        }
        if let Some(tag) = &self.battle_tag {
            out.push((tag.clone(), "<char>"));
            if let Some((name, _)) = tag.split_once('#') {
                out.push((name.to_owned(), "<char>"));
            }
        }
        out
    }
}

/// How to read BugGrabber's `time` (local wall clock, `YYYY/MM/DD HH:MM:SS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeContext {
    /// The machine's UTC offset in seconds (local = UTC + offset).
    pub utc_offset_seconds: i32,
    /// Unix seconds used when `time` is missing or unreadable (file mtime).
    pub fallback_unix: i64,
}

/// One parsed file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BugGrabberScan {
    /// Lower-case hex SHA-256 of the file bytes.
    pub file_sha256: String,
    /// Scrubbed records, merged by key, sorted by key.
    pub records: Vec<AddonErrorRecord>,
    /// Entries dropped (not a table, no usable message, unscrubbable).
    pub skipped: usize,
}

/// Parse `!BugGrabber.lua` bytes. A file without `BugGrabberDB` or without
/// `errors` yields no records; unsupported syntax is an error.
pub fn scan_buggrabber(
    bytes: &[u8],
    scrub: &ScrubContext,
    time: &TimeContext,
) -> Result<BugGrabberScan, WowSvError> {
    if bytes.len() > MAX_SV_BYTES {
        return Err(WowSvError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_SV_BYTES as u64,
        });
    }
    let mut scan = BugGrabberScan {
        file_sha256: sha256_hex(bytes),
        ..BugGrabberScan::default()
    };
    let statements = parse_assignments(bytes)?;
    let Some((_, db)) = statements.iter().rev().find(|(n, _)| n == BUGGRABBER_DB) else {
        return Ok(scan);
    };
    let Some(LuaValue::Table(errors)) = db.get("errors") else {
        return Ok(scan);
    };
    let names = scrub.names();
    let mut merged: BTreeMap<String, AddonErrorRecord> = BTreeMap::new();
    for (_, entry) in errors {
        match read_entry(entry, &names, time) {
            Some(record) => merge(&mut merged, record),
            None => scan.skipped += 1,
        }
    }
    scan.records = merged.into_values().collect();
    Ok(scan)
}

fn merge(into: &mut BTreeMap<String, AddonErrorRecord>, r: AddonErrorRecord) {
    let Some(old) = into.get_mut(&r.key()) else {
        into.insert(r.key(), r);
        return;
    };
    old.count = old.count.saturating_add(r.count).min(ADDON_ERROR_COUNT_MAX);
    old.first_seen_at = old.first_seen_at.clone().min(r.first_seen_at);
    old.last_seen_at = old.last_seen_at.clone().max(r.last_seen_at);
    old.session = old.session.max(r.session);
}

fn clean(raw: &str, names: &[(String, &'static str)], max: usize) -> Option<String> {
    let scrubbed = mask_addresses(&scrub_paths(&scrub_names(&scrub_paths(raw), names)));
    let out = multi_line(&scrubbed, max)?;
    (!has_unscrubbed_path(&out)).then_some(out)
}

/// Never reads `locals` (contract ruling 7).
fn read_entry(
    entry: &LuaValue,
    names: &[(String, &'static str)],
    time: &TimeContext,
) -> Option<AddonErrorRecord> {
    let text = |k: &str| match entry.get(k) {
        Some(LuaValue::Str(s)) => Some(s.as_str()),
        _ => None,
    };
    let int = |k: &str| match entry.get(k) {
        Some(LuaValue::Num(n)) if n.fract() == 0.0 && *n >= 0.0 => Some(*n),
        _ => None,
    };
    let raw_message = text("message")?;
    let message = clean(raw_message, names, ADDON_ERROR_MESSAGE_MAX)?;
    let stack = text("stack").and_then(|s| clean(s, names, ADDON_ERROR_STACK_MAX));
    let addon = addon_name(&message, stack.as_deref());
    let seen = text("time")
        .and_then(|t| local_time_to_unix(t, time.utc_offset_seconds))
        .unwrap_or(time.fallback_unix);
    let at = rfc3339_utc_from_unix_seconds(seen);
    Some(AddonErrorRecord {
        addon,
        message,
        stack,
        first_seen_at: at.clone(),
        last_seen_at: at,
        count: int("counter").map_or(1, |n| n.clamp(1.0, f64::from(ADDON_ERROR_COUNT_MAX)) as u32),
        session: int("session")
            .filter(|n| *n <= f64::from(i32::MAX))
            .map(|n| n as u32),
    })
}

const NOT_THE_CULPRIT: [&str; 2] = ["!BugGrabber", "BugSack"];

/// The first `AddOns<sep><Name><sep>` segment (message first, then stack)
/// that is not BugGrabber itself; `Unknown` when there is none.
fn addon_name(message: &str, stack: Option<&str>) -> String {
    [Some(message), stack]
        .into_iter()
        .flatten()
        .flat_map(addon_segments)
        .find(|n| !NOT_THE_CULPRIT.iter().any(|x| x.eq_ignore_ascii_case(n)))
        .and_then(|n| one_line(n, ADDON_ERROR_ADDON_MAX))
        .unwrap_or_else(|| "Unknown".to_owned())
}

fn addon_segments(s: &str) -> impl Iterator<Item = &str> {
    let b = s.as_bytes();
    let sep = |c: u8| c == b'/' || c == b'\\';
    (0..b.len()).filter_map(move |i| {
        let hit = i + 8 <= b.len()
            && sep(b[i])
            && b[i + 1..i + 7].eq_ignore_ascii_case(b"addons")
            && sep(b[i + 7]);
        if !hit {
            return None;
        }
        let rest = &s[i + 8..];
        let end = rest.find(['/', '\\'])?;
        (end > 0).then(|| &rest[..end])
    })
}

/// `YYYY/MM/DD HH:MM:SS` (or with `-`) local → unix seconds.
fn local_time_to_unix(t: &str, offset: i32) -> Option<i64> {
    let (date, clock) = t.trim().split_once(' ')?;
    let d: Vec<i64> = date
        .split(['/', '-'])
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let c: Vec<i64> = clock
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let ([y, mo, da], [h, mi, se]) = (d.as_slice(), c.as_slice()) else {
        return None;
    };
    let valid = (1970..=9999).contains(y)
        && (1..=12).contains(mo)
        && (1..=31).contains(da)
        && (0..24).contains(h)
        && (0..60).contains(mi)
        && (0..60).contains(se);
    valid.then(|| {
        days_from_civil(*y, *mo, *da) * 86_400 + h * 3600 + mi * 60 + se - i64::from(offset)
    })
}
