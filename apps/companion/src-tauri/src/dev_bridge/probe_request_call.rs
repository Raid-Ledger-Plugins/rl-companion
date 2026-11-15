//! One probe call (`DevBridgeProbeCallSchema`, ROK-2191): the strict shape,
//! then — only while nothing below aborted — the shape refinement (exactly
//! one of fn/presence, presence takes no args/forEach, `$i` needs forEach)
//! and the allowlist on `fn`. Returns the call in output form.

use super::probe_request_rules::{
    check_allowed, int, is_call_id, is_path, js_number, key, strict_keys, string, type_issue, Ctx,
    PathSeg,
};
use serde_json::{Map, Value};

/// The loop-index token a `forEach` call may pass as an argument.
pub const LOOP_TOKEN: &str = "$i";
const CALL_KEYS: &[&str] = &["id", "fn", "presence", "args", "forEach"];
const MAX_ARGS: usize = 8;
const MAX_ARG_CHARS: usize = 128;
const MAX_PATH_CHARS: usize = 160;

/// One call at the current path; its output form (only meaningful when no
/// issue was pushed).
pub fn call(cx: &mut Ctx, v: &Value) -> Value {
    let Some(m) = v.as_object() else {
        type_issue(cx, &[], "object", Some(v));
        return Value::Null;
    };
    let start = cx.issues.len();
    let mut out = Map::new();
    if let Some(id) = string(cx, &["id"], m.get("id"), (0, usize::MAX), is_call_id) {
        out.insert("id".into(), id.into());
    }
    for k in ["fn", "presence"] {
        if let Some(p) = m.get(k) {
            if let Some(s) = string(cx, &[k], Some(p), (0, MAX_PATH_CHARS), is_path) {
                out.insert(k.into(), s.into());
            }
        }
    }
    if let Some(a) = m.get("args") {
        out.insert("args".into(), cx.under(key("args"), |cx| args(cx, a)));
    }
    if let Some(f) = m.get("forEach") {
        out.insert(
            "forEach".into(),
            cx.under(key("forEach"), |cx| for_each(cx, f)),
        );
    }
    strict_keys(cx, m, CALL_KEYS);
    if !cx.aborted_since(start) && check_shape(cx, m) {
        if let Some(f) = m.get("fn").and_then(Value::as_str) {
            check_allowed(cx, f);
        }
    }
    Value::Object(out)
}

/// `args`: ≤8 literals (string ≤128, number, boolean, null).
fn args(cx: &mut Ctx, v: &Value) -> Value {
    let Some(list) = v.as_array() else {
        type_issue(cx, &[], "array", Some(v));
        return Value::Null;
    };
    let out: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, a)| cx.under(PathSeg::Index(i), |cx| arg(cx, a)))
        .collect();
    if list.len() > MAX_ARGS {
        let msg = format!("Too big: expected array to have <={MAX_ARGS} items");
        cx.push(&[], "too_big", msg, false);
    }
    Value::Array(out)
}

/// One literal. A too-long string is the only non-aborted union branch, so
/// Zod reports its `too_big`; anything else off the union is `invalid_union`.
fn arg(cx: &mut Ctx, v: &Value) -> Value {
    match v {
        Value::String(s) => {
            string(cx, &[], Some(v), (0, MAX_ARG_CHARS), |_| true);
            Value::String(s.clone())
        }
        Value::Number(n) => js_number(n),
        Value::Bool(_) | Value::Null => v.clone(),
        _ => {
            cx.push(&[], "invalid_union", "Invalid input".into(), true);
            Value::Null
        }
    }
}

/// `forEach`: strict `{ countFrom: call id, max: int 1..200 }`.
fn for_each(cx: &mut Ctx, v: &Value) -> Value {
    let Some(m) = v.as_object() else {
        type_issue(cx, &[], "object", Some(v));
        return Value::Null;
    };
    let mut out = Map::new();
    let from = m.get("countFrom");
    if let Some(s) = string(cx, &["countFrom"], from, (0, usize::MAX), is_call_id) {
        out.insert("countFrom".into(), s.into());
    }
    if let Some(n) = int(cx, &["max"], m.get("max"), 1, 200) {
        out.insert("max".into(), n.into());
    }
    strict_keys(cx, m, &["countFrom", "max"]);
    Value::Object(out)
}

/// `checkCallShape`: false when fn/presence is not exactly one.
fn check_shape(cx: &mut Ctx, m: &Map<String, Value>) -> bool {
    let has = |k: &str| m.contains_key(k);
    if has("fn") == has("presence") {
        let msg = "Exactly one of fn or presence is required".to_owned();
        cx.push(&[], "custom", msg, false);
        return false;
    }
    if has("presence") && (has("args") || has("forEach")) {
        let msg = "presence takes no args or forEach".to_owned();
        cx.push(&["presence"], "custom", msg, false);
    }
    let uses_loop = m
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(LOOP_TOKEN)));
    if uses_loop && !has("forEach") {
        let msg = "\"$i\" is only valid with forEach".to_owned();
        cx.push(&["args"], "custom", msg, false);
    }
    true
}
