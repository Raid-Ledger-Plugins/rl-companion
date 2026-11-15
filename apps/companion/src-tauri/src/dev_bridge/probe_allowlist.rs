//! The probe function allowlist (ROK-2191; contract `allowlist.json`, the
//! pinned copy of Raid Ledger's `DEV_BRIDGE_PROBE_ALLOWLIST`). rl-companion
//! applies it on `POST /dev/v1/probes`; LedgerLink applies the same table
//! before each call. Order: grammar → deny (case-insensitive) → allow
//! (case-sensitive). Deny always wins. Mirrors `checkDevBridgeProbeFunction`.

use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

const ALLOWLIST_JSON: &str = include_str!("../../../../../contracts/dev-bridge/v1/allowlist.json");

/// One `redact` row: return slots (1-based Lua index) of `fn_name` replaced by
/// `replacement` before a result is stored.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RedactRule {
    #[serde(rename = "fn")]
    pub fn_name: String,
    pub returns: Vec<u32>,
    pub replacement: String,
}

/// One tokenizer test vector.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenizerExample {
    pub segment: String,
    pub words: Vec<String>,
}

/// The shared test vectors every implementation must agree on.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Examples {
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
    pub deny_listed: Vec<String>,
    pub tokenizer: Vec<TokenizerExample>,
}

/// `allowlist.json`, parsed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Allowlist {
    pub allowlist_version: u32,
    pub segment_pattern: String,
    pub max_segments: usize,
    pub allow_prefixes: Vec<String>,
    pub allow_exact: Vec<String>,
    pub deny_words: Vec<String>,
    pub deny_word_pairs: Vec<(String, String)>,
    pub deny_exact: Vec<String>,
    pub redact: Vec<RedactRule>,
    pub examples: Examples,
}

/// Why a path was refused (the `reason` in the issue message).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// More than `maxSegments` segments, or a segment off `segmentPattern`.
    Grammar,
    /// Not a read verb (no allow prefix and not allow-exact).
    NotReadVerb,
    /// A deny word, deny pair or deny-exact name hit.
    Denied,
}

impl Denial {
    /// The reason string Zod's `Function not allowed (<reason>)` carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Denial::Grammar => "grammar",
            Denial::NotReadVerb => "not_read_verb",
            Denial::Denied => "denied",
        }
    }
}

/// The pinned allowlist (parsed once).
pub fn allowlist() -> &'static Allowlist {
    static A: OnceLock<Allowlist> = OnceLock::new();
    A.get_or_init(|| serde_json::from_str(ALLOWLIST_JSON).expect("pinned allowlist.json parses"))
}

/// The `redact` table (return slots replaced before a result is stored).
pub fn redact_rules() -> &'static [RedactRule] {
    &allowlist().redact
}

fn segment_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(&allowlist().segment_pattern).expect("segmentPattern compiles"))
}

/// Split one path segment into words per the `tokenizer` rule
/// (`devBridgeSegmentWords`): `GetCVar` → `Get`,`C`,`Var`.
pub fn segment_words(segment: &str) -> Vec<String> {
    static LOWER_UPPER: OnceLock<Regex> = OnceLock::new();
    static UPPER_UPPER_LOWER: OnceLock<Regex> = OnceLock::new();
    let a = LOWER_UPPER.get_or_init(|| Regex::new("([a-z0-9])([A-Z])").expect("static regex"));
    let b =
        UPPER_UPPER_LOWER.get_or_init(|| Regex::new("([A-Z])([A-Z][a-z])").expect("static regex"));
    segment
        .split('_')
        .flat_map(|piece| {
            let spaced = a.replace_all(piece, "$1 $2");
            let spaced = b.replace_all(&spaced, "$1 $2").into_owned();
            spaced
                .split(' ')
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 1..`maxSegments` dot-separated segments, each matching `segmentPattern`.
pub fn is_grammatical(path: &str) -> bool {
    let segments: Vec<&str> = path.split('.').collect();
    segments.len() <= allowlist().max_segments && segments.iter().all(|s| segment_re().is_match(s))
}

fn has_ci(list: &[String], value: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(value))
}

fn is_denied(segments: &[&str]) -> bool {
    let a = allowlist();
    segments.iter().any(|segment| {
        if has_ci(&a.deny_exact, segment) {
            return true;
        }
        let words = segment_words(segment);
        if words.iter().any(|w| has_ci(&a.deny_words, w)) {
            return true;
        }
        a.deny_word_pairs
            .iter()
            .any(|(x, y)| has_ci(&words, x) && has_ci(&words, y))
    })
}

/// The allow rules alone (`isDevBridgeReadVerb`): the whole path is
/// allow-exact, or the last segment holds no `_` and its first word is an
/// allow prefix.
pub fn is_read_verb(path: &str) -> bool {
    let a = allowlist();
    if a.allow_exact.iter().any(|x| x == path) {
        return true;
    }
    let last = path.rsplit('.').next().unwrap_or("");
    if last.contains('_') {
        return false;
    }
    segment_words(last)
        .first()
        .is_some_and(|w| a.allow_prefixes.iter().any(|p| p == w))
}

/// The verdict for one probe `fn` path. Deny wins over allow.
pub fn check(path: &str) -> Result<(), Denial> {
    if !is_grammatical(path) {
        return Err(Denial::Grammar);
    }
    let segments: Vec<&str> = path.split('.').collect();
    if is_denied(&segments) {
        return Err(Denial::Denied);
    }
    if !is_read_verb(path) {
        return Err(Denial::NotReadVerb);
    }
    Ok(())
}
