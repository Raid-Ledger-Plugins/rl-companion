//! `POST /dev/v1/probes` body validation (ROK-2191): a hand-written mirror of
//! Raid Ledger's `DevBridgeProbeRequestSchema` (contract `$defs/probe-request`,
//! CONTRACT.md "Probe request"), allowlist included, reporting each issue with
//! the path and `code` Zod emits. On success it returns the request in OUTPUT
//! form (defaults filled), from which the queue entry and the probe-detail
//! echo are built.

use super::paging::{clip, json_len, PAGE_MAX_BYTES};
use super::probe_request_call::call;
use super::probe_request_rules::{int, is_display_text, key, strict_keys, string, type_issue, Ctx};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub use super::probe_request_rules::{Issue, PathSeg};

/// `DEV_BRIDGE_PROBE_REQUEST_MAX_BYTES`: the largest derived form's cap.
pub const PROBE_REQUEST_MAX_BYTES: usize = 4_096;
/// `flavor: "auto"` — resolve to the flavor LedgerLink was last seen in.
pub const FLAVOR_AUTO: &str = "auto";
pub const AFTER_EVENTS: &[&str] = &[
    "PLAYER_LOGIN",
    "PLAYER_ENTERING_WORLD",
    "GUILD_ROSTER_UPDATE",
];
const MAX_ISSUES: usize = 50;
const MAX_MESSAGE_CHARS: usize = 512;
const MAX_TITLE_CHARS: usize = 120;
const MAX_CALLS: usize = 40;
const REQUEST_KEYS: &[&str] = &["title", "flavor", "after", "ttlHours", "calls"];
/// The longest flavor folder (34 chars) and a probe id (always 26 chars).
const WORST_FLAVOR: &str = "_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_";
const WORST_PROBE_ID: &str = "00000000000000000000000000";

/// `forEach` in output form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForEach {
    pub count_from: String,
    pub max: u32,
}

/// One call in output form (exactly one of `fn_path` / `presence`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeCall {
    pub id: String,
    #[serde(rename = "fn", default, skip_serializing_if = "Option::is_none")]
    pub fn_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence: Option<String>,
    /// Literals only (string, number, boolean, null).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each: Option<ForEach>,
}

/// A valid probe request in OUTPUT form: `flavor` (`auto` or a folder),
/// `after` and `ttl_hours` always set. Fields serialise in Zod's key order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRequest {
    pub title: String,
    pub flavor: String,
    pub after: String,
    pub ttl_hours: u32,
    pub calls: Vec<ProbeCall>,
}

impl ProbeRequest {
    /// True when the companion must resolve the flavor itself.
    pub fn is_auto_flavor(&self) -> bool {
        self.flavor == FLAVOR_AUTO
    }

    /// The output form; `flavor` replaced when `Some` (the resolved folder).
    pub fn to_output_json(&self, flavor: Option<&str>) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let (Some(f), Some(m)) = (flavor, v.as_object_mut()) {
            m.insert("flavor".into(), f.into());
        }
        v
    }

    /// The `probe-detail.request` echo: output form with the resolved `flavor`.
    pub fn echo_json(&self, flavor: &str) -> Value {
        self.to_output_json(Some(flavor))
    }

    /// One DevQueue.lua entry: `probeId` + output form, `flavor` resolved.
    pub fn queue_entry(&self, probe_id: &str, flavor: &str) -> Value {
        let mut entry = Map::new();
        entry.insert("probeId".into(), probe_id.into());
        if let Value::Object(m) = self.echo_json(flavor) {
            entry.extend(m);
        }
        Value::Object(entry)
    }
}

/// Validate a POST body. `Err` carries every issue Zod would raise, in order.
pub fn validate(body: &Value) -> Result<ProbeRequest, Vec<Issue>> {
    let mut cx = Ctx::default();
    let out = request(&mut cx, body);
    if !cx.issues.is_empty() {
        return Err(cx.issues);
    }
    serde_json::from_value(Value::Object(out)).map_err(|e| {
        let mut cx = Ctx::default();
        cx.push(
            &[],
            "custom",
            format!("Unreadable probe request: {e}"),
            true,
        );
        cx.issues
    })
}

/// The 400 body: `{ error: "invalid_request", issues }`, ≤50 issues and
/// ≤16 KiB serialised (trailing issues dropped to fit).
pub fn error_body(issues: &[Issue]) -> Value {
    let mut list: Vec<Value> = issues
        .iter()
        .take(MAX_ISSUES)
        .map(|i| {
            json!({ "path": i.path, "code": i.code, "message": clip(&i.message, MAX_MESSAGE_CHARS) })
        })
        .collect();
    loop {
        let body = json!({ "error": "invalid_request", "issues": list });
        if json_len(&body) <= PAGE_MAX_BYTES || list.is_empty() {
            return body;
        }
        list.pop();
    }
}

/// Bytes of the largest derived form: `probeId` set, `flavor` at its longest.
pub fn worst_bytes(output: &Map<String, Value>) -> usize {
    let mut m = output.clone();
    m.insert("probeId".into(), WORST_PROBE_ID.into());
    m.insert("flavor".into(), WORST_FLAVOR.into());
    json_len(&Value::Object(m))
}

fn request(cx: &mut Ctx, v: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(m) = v.as_object() else {
        type_issue(cx, &[], "object", Some(v));
        return out;
    };
    let title = string(
        cx,
        &["title"],
        m.get("title"),
        (1, MAX_TITLE_CHARS),
        is_display_text,
    );
    if let Some(t) = title {
        out.insert("title".into(), t.into());
    }
    out.insert("flavor".into(), flavor(cx, m.get("flavor")).into());
    out.insert("after".into(), after(cx, m.get("after")).into());
    let ttl = match m.get("ttlHours") {
        None => Some(72),
        v => int(cx, &["ttlHours"], v, 1, 168),
    };
    if let Some(t) = ttl {
        out.insert("ttlHours".into(), t.into());
    }
    if let Some(c) = calls(cx, m.get("calls")) {
        out.insert("calls".into(), c);
    }
    strict_keys(cx, m, REQUEST_KEYS);
    if !cx.aborted_since(0) {
        check_request(cx, &out);
    }
    out
}

/// `auto` (default) or a flavor folder; a bad string is the regex branch's
/// `invalid_format`, a non-string `invalid_union`.
fn flavor(cx: &mut Ctx, v: Option<&Value>) -> String {
    match v {
        None => FLAVOR_AUTO.to_owned(),
        Some(Value::String(s)) => {
            if s != FLAVOR_AUTO && !super::health::is_flavor(s) {
                let msg = "Invalid string: must match pattern /^_[a-z0-9_]{1,32}_$/".to_owned();
                cx.push(&["flavor"], "invalid_format", msg, false);
            }
            s.clone()
        }
        Some(_) => {
            cx.push(&["flavor"], "invalid_union", "Invalid input".into(), true);
            String::new()
        }
    }
}

fn after(cx: &mut Ctx, v: Option<&Value>) -> String {
    match v {
        None => AFTER_EVENTS[0].to_owned(),
        Some(Value::String(s)) if AFTER_EVENTS.contains(&s.as_str()) => s.clone(),
        Some(_) => {
            let msg = format!("Invalid option: expected one of {}", AFTER_EVENTS.join("|"));
            cx.push(&["after"], "invalid_value", msg, true);
            String::new()
        }
    }
}

fn calls(cx: &mut Ctx, v: Option<&Value>) -> Option<Value> {
    let Some(list) = v.and_then(Value::as_array) else {
        type_issue(cx, &["calls"], "array", v);
        return None;
    };
    let out = cx.under(key("calls"), |cx| {
        let out: Vec<Value> = list
            .iter()
            .enumerate()
            .map(|(i, c)| cx.under(PathSeg::Index(i), |cx| call(cx, c)))
            .collect();
        if list.is_empty() {
            let msg = "Too small: expected array to have >=1 items".to_owned();
            cx.push(&[], "too_small", msg, false);
        }
        if list.len() > MAX_CALLS {
            let msg = format!("Too big: expected array to have <={MAX_CALLS} items");
            cx.push(&[], "too_big", msg, false);
        }
        out
    });
    Some(Value::Array(out))
}

/// `checkDevBridgeProbeRequest`: unique ids, backward `countFrom`, 4 KiB cap.
fn check_request(cx: &mut Ctx, out: &Map<String, Value>) {
    let empty = Vec::new();
    let list = out.get("calls").and_then(Value::as_array).unwrap_or(&empty);
    let mut seen: HashMap<&str, bool> = HashMap::new();
    for (i, c) in list.iter().enumerate() {
        let id = c.get("id").and_then(Value::as_str).unwrap_or("");
        let at = |rest: &[&str]| {
            let mut p = vec![key("calls"), PathSeg::Index(i)];
            p.extend(rest.iter().map(|k| key(k)));
            p
        };
        if seen.contains_key(id) {
            cx.push_at(at(&["id"]), "custom", "Duplicate call id".into(), false);
        }
        let from = c.pointer("/forEach/countFrom").and_then(Value::as_str);
        if from.is_some_and(|f| seen.get(f) != Some(&true)) {
            let msg = "countFrom must name an earlier fn call".to_owned();
            cx.push_at(at(&["forEach", "countFrom"]), "custom", msg, false);
        }
        seen.insert(id, c.get("fn").is_some());
    }
    let bytes = worst_bytes(out);
    if bytes > PROBE_REQUEST_MAX_BYTES {
        let msg = format!(
            "Probe request is {bytes} bytes as a queue entry with the longest flavor; the cap is {PROBE_REQUEST_MAX_BYTES}"
        );
        cx.push(&[], "custom", msg, false);
    }
}
