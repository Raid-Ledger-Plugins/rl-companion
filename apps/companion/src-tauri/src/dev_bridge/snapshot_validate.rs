//! Rust-side check of a `ledgerlink-snapshot` (ROK-2072 §4, §6) before it is
//! cached. Mirrors `$defs/ledgerlink-snapshot` (strict keys, types, enums,
//! lengths) plus the refinements `schema.json` cannot express: the player-GUID
//! guard, unique `apis[].path`, `type` iff `status: presence`, D4 (a
//! name-bearing fn is reported as `presence` only), and the item /
//! head / whole byte caps. String lengths count UTF-16 units, as Zod does.
//! An error never echoes a GUID: a GUID-bearing key is reported as `<guid>`.

use super::logbuf::parse_utc;
use super::paging::json_len;
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::OnceLock;

pub const SNAPSHOT_MAX_BYTES: usize = 49_152;
pub const SNAPSHOT_HEAD_MAX_BYTES: usize = 4_096;
pub const RESULT_ITEM_MAX_BYTES: usize = 3_072;
/// Zod 4 `.int()` bounds: ±(2^53 − 1).
const MAX_SAFE_INT: i64 = (1 << 53) - 1;
const LUA_TYPES: &[&str] = &[
    "nil", "function", "table", "string", "number", "boolean", "userdata", "thread",
];

/// D4: `DEV_BRIDGE_SNAPSHOT_NAME_BEARING_FNS` — values carry a name, realm,
/// guild or GUID, so a snapshot reports them as `status: presence` only.
pub const NAME_BEARING_FNS: &[&str] = &[
    "UnitGUID",
    "GetUnitName",
    "UnitFullName",
    "UnitName",
    "GetRealmName",
    "GetNormalizedRealmName",
    "GetGuildInfo",
    "GetGuildRosterInfo",
    "GetGuildRosterLastOnline",
];

type Check = Result<(), String>;

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("static regex"))
}

fn guid_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"Player-\d{1,5}-[0-9A-Fa-f]{8}")
}

/// `k` for an error path or message, any player GUID in it replaced by `<guid>`
/// (the error is logged; a GUID must never reach a log).
fn key_label(k: &str) -> String {
    guid_re().replace_all(k, "<guid>").into_owned()
}

fn api_path_ok(s: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    s.len() <= 96
        && re(
            &R,
            r"^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*){0,2}$",
        )
        .is_match(s)
}

fn err<T>(at: &str, msg: &str) -> Result<T, String> {
    Err(format!("{at}: {msg}"))
}

/// The object at `at`, holding no keys outside `allowed` and every `required`.
fn obj<'a>(
    v: &'a Value,
    at: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let Some(m) = v.as_object() else {
        return err(at, "expected an object");
    };
    if let Some(k) = m
        .keys()
        .find(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return err(at, &format!("unrecognized key {}", key_label(k)));
    }
    if let Some(k) = required.iter().find(|k| !m.contains_key(**k)) {
        return err(at, &format!("missing {k}"));
    }
    Ok(m)
}

fn text(m: &Map<String, Value>, k: &str, at: &str, max: usize) -> Check {
    match m.get(k).and_then(Value::as_str) {
        Some(s) if s.encode_utf16().count() <= max => Ok(()),
        _ => err(
            &format!("{at}.{k}"),
            &format!("expected a string of at most {max}"),
        ),
    }
}

fn opt_text(m: &Map<String, Value>, k: &str, at: &str, max: usize) -> Check {
    if m.contains_key(k) {
        text(m, k, at, max)
    } else {
        Ok(())
    }
}

fn int_in(v: Option<&Value>, lo: i64, hi: i64, nullable: bool) -> bool {
    match v {
        Some(Value::Null) => nullable,
        Some(n) => n.as_i64().is_some_and(|n| (lo..=hi).contains(&n)),
        None => false,
    }
}

fn one_of(v: Option<&Value>, allowed: &[&str], nullable: bool) -> bool {
    match v {
        Some(Value::Null) => nullable,
        Some(Value::String(s)) => allowed.contains(&s.as_str()),
        _ => false,
    }
}

fn arr<'a>(m: &'a Map<String, Value>, k: &str, max: usize) -> Result<&'a Vec<Value>, String> {
    match m.get(k).and_then(Value::as_array) {
        Some(a) if a.len() <= max => Ok(a),
        _ => err(k, &format!("expected an array of at most {max}")),
    }
}

fn is_true(m: &Map<String, Value>, k: &str) -> bool {
    m.get(k).is_none_or(|v| v == &Value::Bool(true))
}

/// `DevBridgeProbeValueSchema`: scalars (strings ≤ 512) nested ≤ 3 levels in
/// arrays (≤ 40) or objects (≤ 40 keys of ≤ 128), like Zod's `tableOf`.
fn probe_value(v: &Value, depth: usize) -> bool {
    match v {
        Value::String(s) => s.encode_utf16().count() <= 512,
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::Array(a) => {
            depth < 3 && a.len() <= 40 && a.iter().all(|x| probe_value(x, depth + 1))
        }
        Value::Object(o) => {
            depth < 3
                && o.len() <= 40
                && o.iter()
                    .all(|(k, x)| k.encode_utf16().count() <= 128 && probe_value(x, depth + 1))
        }
    }
}

fn client(v: &Value) -> Check {
    let keys = [
        "version",
        "build",
        "buildDate",
        "interface",
        "tocInterface",
        "locale",
        "region",
    ];
    let m = obj(v, "client", &keys, &[])?;
    text(m, "version", "client", 32)?;
    text(m, "build", "client", 16)?;
    text(m, "buildDate", "client", 32)?;
    text(m, "locale", "client", 8)?;
    let ok = int_in(m.get("interface"), 0, 9_999_999, false)
        && int_in(m.get("tocInterface"), 0, 9_999_999, true)
        && int_in(m.get("region"), -MAX_SAFE_INT, MAX_SAFE_INT, true);
    if ok {
        Ok(())
    } else {
        err("client", "interface/tocInterface/region out of range")
    }
}

fn character(v: &Value) -> Check {
    static CLASS: OnceLock<Regex> = OnceLock::new();
    let m = obj(
        v,
        "character",
        &["class", "level", "faction", "inGuild"],
        &[],
    )?;
    let class_ok = match m.get("class") {
        Some(Value::Null) => true,
        Some(Value::String(s)) => re(&CLASS, "^[A-Z]{2,16}$").is_match(s),
        _ => false,
    };
    let ok = class_ok
        && int_in(m.get("level"), 1, 100, true)
        && one_of(m.get("faction"), &["Alliance", "Horde", "Neutral"], true)
        && m.get("inGuild").is_some_and(Value::is_boolean);
    if ok {
        Ok(())
    } else {
        err("character", "bad class/level/faction/inGuild")
    }
}

fn ruleset(v: &Value) -> Check {
    static NAME: OnceLock<Regex> = OnceLock::new();
    let keys = [
        "isGameRuleActive",
        "enumGameRule",
        "rules",
        "rulesTruncated",
        "ledgerLink",
    ];
    let m = obj(v, "ruleset", &keys, &[])?;
    if !one_of(m.get("isGameRuleActive"), LUA_TYPES, false)
        || !one_of(m.get("enumGameRule"), LUA_TYPES, false)
    {
        return err("ruleset", "expected Lua type names");
    }
    for (i, r) in arr(m, "rules", 64)?.iter().enumerate() {
        let at = format!("ruleset.rules.{i}");
        let rm = obj(r, &at, &["name", "value", "active"], &["error"])?;
        let name_ok = rm
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|s| re(&NAME, "^[A-Za-z_][A-Za-z0-9_]{0,63}$").is_match(s));
        let value_ok = match rm.get("value") {
            Some(Value::Number(_)) => true,
            Some(Value::String(s)) => s.encode_utf16().count() <= 64,
            _ => false,
        };
        let active_ok = rm
            .get("active")
            .is_some_and(|a| a.is_boolean() || a.is_null());
        if !(name_ok && value_ok && active_ok) {
            return err(&at, "bad name/value/active");
        }
        opt_text(rm, "error", &at, 256)?;
    }
    if !m.get("rulesTruncated").is_some_and(Value::is_boolean) {
        return err("ruleset.rulesTruncated", "expected a boolean");
    }
    let ll = obj(
        m.get("ledgerLink").unwrap_or(&Value::Null),
        "ruleset.ledgerLink",
        &["value", "source"],
        &[],
    )?;
    let ok = one_of(
        ll.get("value"),
        &["normal", "pvp", "roleplaying", "hardcore"],
        true,
    ) && one_of(ll.get("source"), &["set", "detected"], true);
    if ok {
        Ok(())
    } else {
        err("ruleset.ledgerLink", "bad value/source")
    }
}

fn call(v: &Value, at: &str) -> Check {
    let m = obj(
        v,
        at,
        &["fn", "status"],
        &["args", "type", "values", "error", "truncated"],
    )?;
    if !m.get("fn").and_then(Value::as_str).is_some_and(api_path_ok) {
        return err(at, "bad fn");
    }
    if let Some(a) = m.get("args") {
        let ok = a.as_array().is_some_and(|a| {
            a.len() <= 8
                && a.iter().all(|x| {
                    !x.is_array()
                        && !x.is_object()
                        && x.as_str().is_none_or(|s| s.encode_utf16().count() <= 128)
                })
        });
        if !ok {
            return err(at, "bad args");
        }
    }
    let presence = m.get("status").and_then(Value::as_str) == Some("presence");
    if !one_of(
        m.get("status"),
        &["ok", "missing", "error", "presence"],
        false,
    ) {
        return err(at, "bad status");
    }
    if presence != m.contains_key("type") || (presence && m.contains_key("values")) {
        return err(at, "type (and no values) iff status is presence");
    }
    let fn_name = m.get("fn").and_then(Value::as_str).unwrap_or_default();
    if !presence && NAME_BEARING_FNS.contains(&fn_name) {
        return err(at, "name-bearing fn is reported as presence only (D4)");
    }
    if m.contains_key("type") && !one_of(m.get("type"), LUA_TYPES, false) {
        return err(at, "bad type");
    }
    if let Some(vals) = m.get("values") {
        let ok = vals
            .as_array()
            .is_some_and(|a| a.len() <= 40 && a.iter().all(|x| probe_value(x, 0)));
        if !ok {
            return err(at, "bad values");
        }
    }
    opt_text(m, "error", at, 512)?;
    if is_true(m, "truncated") {
        Ok(())
    } else {
        err(at, "truncated is true when present")
    }
}

fn api(v: &Value, at: &str, seen: &mut HashSet<String>) -> Check {
    let m = obj(v, at, &["path", "type"], &[])?;
    let path = m
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| api_path_ok(p));
    let Some(path) = path else {
        return err(at, "bad path");
    };
    if !one_of(m.get("type"), LUA_TYPES, false) {
        return err(at, "bad type");
    }
    if seen.insert(path.to_owned()) {
        Ok(())
    } else {
        err(at, "duplicate path")
    }
}

fn section(v: &Value, at: &str) -> Check {
    static ID: OnceLock<Regex> = OnceLock::new();
    let m = obj(v, at, &["id", "status", "lines"], &["error", "truncated"])?;
    let id_ok = m
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|s| re(&ID, "^S[0-9]{1,2}$").is_match(s));
    let lines_ok = m.get("lines").and_then(Value::as_array).is_some_and(|l| {
        l.len() <= 40
            && l.iter()
                .all(|x| x.as_str().is_some_and(|s| s.encode_utf16().count() <= 256))
    });
    if !(id_ok && lines_ok && one_of(m.get("status"), &["ok", "error", "skipped"], false)) {
        return err(at, "bad id/status/lines");
    }
    opt_text(m, "error", at, 512)?;
    if is_true(m, "truncated") {
        Ok(())
    } else {
        err(at, "truncated is true when present")
    }
}

/// The first string (or key) anywhere in `v` holding a player GUID.
fn guid_at(v: &Value, at: &str) -> Option<String> {
    match v {
        Value::String(s) => guid_re().is_match(s).then(|| at.to_owned()),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, x)| guid_at(x, &format!("{at}.{i}"))),
        Value::Object(o) => o.iter().find_map(|(k, x)| {
            let label = key_label(k);
            let here = if at.is_empty() {
                label
            } else {
                format!("{at}.{label}")
            };
            if guid_re().is_match(k) {
                Some(here)
            } else {
                guid_at(x, &here)
            }
        }),
        _ => None,
    }
}

const ROOT_KEYS: [&str; 11] = [
    "snapshotVersion",
    "addonVersion",
    "capturedAt",
    "trigger",
    "client",
    "character",
    "ruleset",
    "calls",
    "apis",
    "sections",
    "truncated",
];

/// The object shape + per-item refinements; `slice` skips the head and
/// whole-size caps (a page carries a slice of the arrays).
fn check(v: &Value, slice: bool) -> Check {
    let m = obj(v, "root", &ROOT_KEYS, &[])?;
    if m.get("snapshotVersion").and_then(Value::as_i64) != Some(1) {
        return err("snapshotVersion", "expected 1");
    }
    if !m
        .get("addonVersion")
        .and_then(Value::as_str)
        .is_some_and(|s| (1..=64).contains(&s.encode_utf16().count()))
    {
        return err("addonVersion", "expected 1–64 chars");
    }
    let ts = m.get("capturedAt").and_then(Value::as_str);
    if !ts.is_some_and(|s| s.len() <= 24 && parse_utc(s).is_some()) {
        return err("capturedAt", "expected an RFC 3339 UTC time");
    }
    if !one_of(m.get("trigger"), &["logout"], false)
        || !m.get("truncated").is_some_and(Value::is_boolean)
    {
        return err("trigger", "bad trigger/truncated");
    }
    client(&m["client"])?;
    character(&m["character"])?;
    ruleset(&m["ruleset"])?;
    let mut seen = HashSet::new();
    let lists: [(&str, usize); 3] = [("calls", 40), ("apis", 160), ("sections", 9)];
    for (k, max) in lists {
        for (i, item) in arr(m, k, max)?.iter().enumerate() {
            let at = format!("{k}.{i}");
            match k {
                "calls" => call(item, &at)?,
                "apis" => api(item, &at, &mut seen)?,
                _ => section(item, &at)?,
            }
            if json_len(item) > RESULT_ITEM_MAX_BYTES {
                return err(&at, "item over 3 KiB");
            }
        }
    }
    if let Some(at) = guid_at(v, "") {
        return err(&at, "holds a player GUID");
    }
    if !slice && json_len(&head(v)) > SNAPSHOT_HEAD_MAX_BYTES {
        return err("root", "head over 4 KiB");
    }
    if !slice && json_len(v) > SNAPSHOT_MAX_BYTES {
        return err("root", "snapshot over 48 KiB");
    }
    Ok(())
}

/// The snapshot without `calls` / `apis` / `sections`.
pub fn head(v: &Value) -> Value {
    let mut h = v.clone();
    if let Some(o) = h.as_object_mut() {
        for k in ["calls", "apis", "sections"] {
            o.remove(k);
        }
    }
    h
}

/// A stored snapshot: every check (`$defs/ledgerlink-snapshot`).
pub fn validate_snapshot(v: &Value) -> Check {
    check(v, false)
}

/// A page's `snapshot` slice: shape, per-item and GUID checks only.
pub fn validate_slice(v: &Value) -> Check {
    check(v, true)
}
