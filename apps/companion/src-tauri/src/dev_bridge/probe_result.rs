//! Rust-side check of a `probe-result` (ROK-2191) — the JSON string LedgerLink
//! stores in `LedgerLinkDB.dev.probes[probeId]`. Mirrors
//! `DevBridgeProbeResultSchema` (strict keys, enums, UTF-16 string lengths,
//! the ULID / call-id / version patterns, values nested ≤ 3 deep with ≤ 40
//! entries per table) plus its refinement: each item (a call without
//! `iterations`, or one iteration) ≤ 3 KiB and the whole result ≤ 64 KiB,
//! measured as serialised JSON bytes. The error is the human reason a probe
//! is marked `rejected` with: a path and a rule, never a value (≤ 512 chars).

use super::logbuf::parse_utc;
use super::paging::json_len;
use super::snapshot_validate::RESULT_ITEM_MAX_BYTES;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// `DEV_BRIDGE_PROBE_RESULT_MAX_BYTES`.
pub const PROBE_RESULT_MAX_BYTES: usize = 65_536;
/// Longest rejection reason (the probe-detail `error` field cap).
pub const REASON_MAX_CHARS: usize = 512;
const TABLE_MAX_ENTRIES: usize = 40;
const TABLE_MAX_DEPTH: usize = 3;
const STATUSES: &[&str] = &["ok", "missing", "error", "denied"];

type Check = Result<(), String>;

/// Parse `raw_json` and check it is a valid `probe-result` whose `probeId`
/// equals `expected_probe_id` (the SavedVariables key it was stored under).
pub fn validate_result(raw_json: &str, expected_probe_id: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw_json).map_err(|e| {
        format!(
            "result: not valid JSON (line {}, column {})",
            e.line(),
            e.column()
        )
    })?;
    match check(&v, expected_probe_id) {
        Ok(()) => Ok(v),
        Err(reason) => Err(reason.chars().take(REASON_MAX_CHARS).collect()),
    }
}

fn check(v: &Value, expected: &str) -> Check {
    let keys = [
        "probeId",
        "capturedAt",
        "addonVersion",
        "calls",
        "truncated",
    ];
    let m = obj(v, "result", &keys, &[])?;
    let id = m.get("probeId").and_then(Value::as_str).unwrap_or_default();
    if !rl_collectors::is_probe_id(id) {
        return err("probeId", "expected a 26-char ULID");
    }
    if id != expected {
        return err(
            "probeId",
            "does not match the SavedVariables key it was stored under",
        );
    }
    let at = m.get("capturedAt").and_then(Value::as_str);
    if !at.is_some_and(|s| s.encode_utf16().count() <= 24 && parse_utc(s).is_some()) {
        return err(
            "capturedAt",
            "expected an RFC 3339 UTC timestamp of at most 24 chars",
        );
    }
    let ver = m.get("addonVersion").and_then(Value::as_str);
    if !ver.is_some_and(|s| s.encode_utf16().count() <= 48 && version_re().is_match(s)) {
        return err("addonVersion", "expected a version like 1.2.3[-tag]");
    }
    if !m.get("truncated").is_some_and(Value::is_boolean) {
        return err("truncated", "expected a boolean");
    }
    let calls = array(m.get("calls"), "calls", 40)?;
    for (c, call) in calls.iter().enumerate() {
        call_result(call, &format!("calls.{c}"))?;
    }
    sizes(calls)?;
    let bytes = json_len(v);
    if bytes > PROBE_RESULT_MAX_BYTES {
        return err(
            "result",
            &format!("is {bytes} bytes; the cap is {PROBE_RESULT_MAX_BYTES}"),
        );
    }
    Ok(())
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("static regex"))
}

fn version_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"^[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4}(-[0-9A-Za-z.-]{1,32})?$",
    )
}

fn call_id_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^[A-Za-z][A-Za-z0-9_]{0,31}$")
}

fn err<T>(at: &str, msg: &str) -> Result<T, String> {
    Err(format!("{at}: {msg}"))
}

/// The object at `at`, holding every `required` key and nothing outside
/// `required` + `optional`. An unknown key is reported by position, not name.
fn obj<'a>(
    v: &'a Value,
    at: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let Some(m) = v.as_object() else {
        return err(at, "expected an object");
    };
    let known = |k: &String| required.contains(&k.as_str()) || optional.contains(&k.as_str());
    if m.keys().any(|k| !known(k)) {
        return err(at, "unrecognized key");
    }
    if let Some(k) = required.iter().find(|k| !m.contains_key(**k)) {
        return err(at, &format!("missing {k}"));
    }
    Ok(m)
}

fn array<'a>(v: Option<&'a Value>, at: &str, max: usize) -> Result<&'a Vec<Value>, String> {
    match v.and_then(Value::as_array) {
        Some(a) if a.len() <= max => Ok(a),
        _ => err(at, &format!("expected an array of at most {max}")),
    }
}

/// `{ id, status, values?, error?, iterations?, truncated? }`.
fn call_result(v: &Value, at: &str) -> Check {
    let optional = ["values", "error", "iterations", "truncated"];
    let m = obj(v, at, &["id", "status"], &optional)?;
    let id = m.get("id").and_then(Value::as_str);
    if !id.is_some_and(|s| call_id_re().is_match(s)) {
        return err(&format!("{at}.id"), "expected a call id");
    }
    outcome(m, at)?;
    if m.get("truncated").is_some_and(|t| t != &Value::Bool(true)) {
        return err(&format!("{at}.truncated"), "expected true when present");
    }
    if let Some(its) = m.get("iterations") {
        let at = format!("{at}.iterations");
        for (i, it) in array(Some(its), &at, 200)?.iter().enumerate() {
            iteration(it, &format!("{at}.{i}"))?;
        }
    }
    Ok(())
}

/// `{ i, status, values?, error? }`, `i` an integer 1..=200.
fn iteration(v: &Value, at: &str) -> Check {
    let m = obj(v, at, &["i", "status"], &["values", "error"])?;
    let i = m.get("i").and_then(Value::as_f64);
    if !i.is_some_and(|f| f.fract() == 0.0 && (1.0..=200.0).contains(&f)) {
        return err(&format!("{at}.i"), "expected an integer from 1 to 200");
    }
    outcome(m, at)
}

/// `status` (closed enum), `values?` (≤ 40 probe values), `error?` (≤ 512).
fn outcome(m: &Map<String, Value>, at: &str) -> Check {
    let status = m.get("status").and_then(Value::as_str);
    if !status.is_some_and(|s| STATUSES.contains(&s)) {
        return err(
            &format!("{at}.status"),
            "expected ok, missing, error or denied",
        );
    }
    if let Some(values) = m.get("values") {
        let at = format!("{at}.values");
        for (k, x) in array(Some(values), &at, 40)?.iter().enumerate() {
            probe_value(x, 0, &format!("{at}.{k}"))?;
        }
    }
    match m.get("error") {
        Some(Value::String(s)) if s.encode_utf16().count() > 512 => {
            err(&format!("{at}.error"), "longer than 512 characters")
        }
        Some(Value::String(_)) | None => Ok(()),
        Some(_) => err(&format!("{at}.error"), "expected a string"),
    }
}

/// `DevBridgeProbeValueSchema`: a leaf (string ≤ 512, number, boolean, null)
/// or a table (array, or object with keys ≤ 128) at `depth` < 3, ≤ 40 entries.
fn probe_value(v: &Value, depth: usize, at: &str) -> Check {
    let table = |len: usize| -> Check {
        if depth >= TABLE_MAX_DEPTH {
            return err(at, "tables nest at most 3 deep");
        }
        if len > TABLE_MAX_ENTRIES {
            return err(at, "a table holds at most 40 entries");
        }
        Ok(())
    };
    match v {
        Value::String(s) if s.encode_utf16().count() > 512 => err(at, "longer than 512 characters"),
        Value::String(_) | Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::Array(a) => {
            table(a.len())?;
            a.iter()
                .enumerate()
                .try_for_each(|(k, x)| probe_value(x, depth + 1, &format!("{at}.{k}")))
        }
        Value::Object(o) => {
            table(o.len())?;
            o.iter().enumerate().try_for_each(|(k, (key, x))| {
                let at = format!("{at}.#{k}");
                if key.encode_utf16().count() > 128 {
                    return err(&at, "table key longer than 128 characters");
                }
                probe_value(x, depth + 1, &at)
            })
        }
    }
}

/// Each item (a call without `iterations`, or one iteration) ≤ 3 KiB.
fn sizes(calls: &[Value]) -> Check {
    for (c, call) in calls.iter().enumerate() {
        let bytes = json_len(&call_head(call));
        if bytes > RESULT_ITEM_MAX_BYTES {
            return item_too_big(&format!("calls.{c}"), bytes);
        }
        let its = call.get("iterations").and_then(Value::as_array);
        for (i, it) in its.into_iter().flatten().enumerate() {
            let bytes = json_len(it);
            if bytes > RESULT_ITEM_MAX_BYTES {
                return item_too_big(&format!("calls.{c}.iterations.{i}"), bytes);
            }
        }
    }
    Ok(())
}

fn item_too_big(at: &str, bytes: usize) -> Check {
    err(
        at,
        &format!("result item is {bytes} bytes; the cap is {RESULT_ITEM_MAX_BYTES}"),
    )
}

/// The call without its `iterations` — one paging item.
pub fn call_head(call: &Value) -> Value {
    let mut head = call.clone();
    if let Some(m) = head.as_object_mut() {
        m.remove("iterations");
    }
    head
}
