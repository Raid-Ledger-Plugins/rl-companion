//! Zod 4 primitives for `probe_request` (ROK-2191): each check pushes the
//! issue Zod would, with Zod's `code`, and marks it aborting when Zod does
//! (type/union/enum/unknown-key issues). A refinement only runs while no
//! aborting issue sits below it — Zod 4 skips `superRefine` then too.
//! String lengths count UTF-16 units, as Zod does.

use super::probe_allowlist;
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Number, Value};
use std::sync::OnceLock;

/// One element of an issue path: an object key or an array index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}

/// One `error.issues` item: `{ path, code, message }` as Zod reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub path: Vec<PathSeg>,
    pub code: &'static str,
    pub message: String,
    /// Zod's non-continuable issue: refinements above it are skipped.
    #[serde(skip)]
    pub abort: bool,
}

impl Issue {
    /// The path dot-joined (`calls.0.fn`; `""` at the root), as `EXPECTED.json` writes it.
    pub fn dotted(&self) -> String {
        let parts: Vec<String> = self
            .path
            .iter()
            .map(|s| match s {
                PathSeg::Key(k) => k.clone(),
                PathSeg::Index(i) => i.to_string(),
            })
            .collect();
        parts.join(".")
    }
}

/// The issue list plus the path of the value being checked.
#[derive(Default)]
pub struct Ctx {
    pub issues: Vec<Issue>,
    pub at: Vec<PathSeg>,
}

impl Ctx {
    pub fn push(&mut self, rel: &[&str], code: &'static str, msg: String, abort: bool) {
        let mut path = self.at.clone();
        path.extend(rel.iter().map(|k| key(k)));
        self.push_at(path, code, msg, abort);
    }

    /// Push an issue at an absolute `path`.
    pub fn push_at(&mut self, path: Vec<PathSeg>, code: &'static str, msg: String, abort: bool) {
        self.issues.push(Issue {
            path,
            code,
            message: msg,
            abort,
        });
    }

    /// Run `f` with `seg` appended to the current path.
    pub fn under<T>(&mut self, seg: PathSeg, f: impl FnOnce(&mut Self) -> T) -> T {
        self.at.push(seg);
        let out = f(self);
        self.at.pop();
        out
    }

    /// True when an aborting issue was pushed at or after `from`.
    pub fn aborted_since(&self, from: usize) -> bool {
        self.issues.iter().skip(from).any(|i| i.abort)
    }
}

pub fn key(k: &str) -> PathSeg {
    PathSeg::Key(k.to_owned())
}

fn kind(v: Option<&Value>) -> &'static str {
    match v {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

pub fn type_issue(cx: &mut Ctx, rel: &[&str], want: &str, got: Option<&Value>) {
    let msg = format!("Invalid input: expected {want}, received {}", kind(got));
    cx.push(rel, "invalid_type", msg, true);
}

pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `z.string().min(min).max(max)` + a pattern check (`ok`); `None` (and an
/// aborting `invalid_type`) when `v` is not a string.
pub fn string<'a>(
    cx: &mut Ctx,
    rel: &[&str],
    v: Option<&'a Value>,
    (min, max): (usize, usize),
    ok: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    let Some(s) = v.and_then(Value::as_str) else {
        type_issue(cx, rel, "string", v);
        return None;
    };
    let len = utf16_len(s);
    if len < min {
        let msg = format!("Too small: expected string to have >={min} characters");
        cx.push(rel, "too_small", msg, false);
    }
    if len > max {
        let msg = format!("Too big: expected string to have <={max} characters");
        cx.push(rel, "too_big", msg, false);
    }
    if !ok(s) {
        cx.push(
            rel,
            "invalid_format",
            "Invalid string: must match pattern".into(),
            false,
        );
    }
    Some(s)
}

/// `z.number().int().min(min).max(max)`; returns the integer when it is one.
pub fn int(cx: &mut Ctx, rel: &[&str], v: Option<&Value>, min: i64, max: i64) -> Option<i64> {
    let Some(n) = v.and_then(Value::as_f64) else {
        type_issue(cx, rel, "number", v);
        return None;
    };
    if n.fract() != 0.0 || !n.is_finite() {
        cx.push(
            rel,
            "invalid_type",
            "Invalid input: expected int, received number".into(),
            true,
        );
        return None;
    }
    if n < min as f64 {
        cx.push(
            rel,
            "too_small",
            format!("Too small: expected number to be >={min}"),
            false,
        );
    }
    if n > max as f64 {
        cx.push(
            rel,
            "too_big",
            format!("Too big: expected number to be <={max}"),
            false,
        );
    }
    Some(n as i64)
}

/// A JSON number as `JSON.stringify` writes it where that matters for the
/// byte cap: an integral float (`1.0`) becomes an integer (`1`).
pub fn js_number(n: &Number) -> Value {
    match n.as_f64() {
        Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 => {
            Value::from(f as i64)
        }
        _ => Value::Number(n.clone()),
    }
}

/// Keys of `m` outside `known`, as one aborting `unrecognized_keys` issue.
pub fn strict_keys(cx: &mut Ctx, m: &Map<String, Value>, known: &[&str]) {
    let extra: Vec<&str> = m
        .keys()
        .map(String::as_str)
        .filter(|k| !known.contains(k))
        .collect();
    if extra.is_empty() {
        return;
    }
    let msg = format!("Unrecognized key(s) in object: {}", extra.join(", "));
    cx.push(&[], "unrecognized_keys", msg, true);
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("static regex"))
}

/// `DevBridgeCallIdSchema`.
pub fn is_call_id(s: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, "^[A-Za-z][A-Za-z0-9_]{0,31}$").is_match(s)
}

/// The `fn` / `presence` path pattern (1–3 identifiers).
pub fn is_path(s: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*){0,2}$",
    )
    .is_match(s)
}

/// JS `\s` (WhiteSpace + LineTerminator).
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `COMPANION_DISPLAY_TEXT_RE`: not blank, no control / line-separator / bidi characters.
pub fn is_display_text(s: &str) -> bool {
    let bad = |c: char| {
        c.is_control()
            || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
    };
    !s.chars().all(js_space) && !s.chars().any(bad)
}

/// `fn` must pass the allowlist; issue at `fn` (code `custom`).
pub fn check_allowed(cx: &mut Ctx, fn_path: &str) {
    if let Err(d) = probe_allowlist::check(fn_path) {
        let msg = format!("Function not allowed ({})", d.as_str());
        cx.push(&["fn"], "custom", msg, false);
    }
}
