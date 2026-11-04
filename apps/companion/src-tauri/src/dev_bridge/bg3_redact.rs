//! Redaction for the BG3 dev-bridge bodies (`dev-bridge/v1` ROK-2077): values
//! that do not fit the contract's shapes are dropped, never rewritten, and a
//! final guard refuses any body that still holds a GUID, a Larian handle or a
//! run of ≥ 10 digits (a SteamID64 / profile id). No name, path or GUID leaves.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static STATS_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9]{1,5}_[A-Za-z0-9_]{1,57}$").expect("regex"));
static CLASS_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.]+$").expect("regex"));
static TYPE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[a-z][a-z0-9_]*(\.[A-Za-z0-9_]+){2,}|E[A-Z][A-Za-z0-9]{1,80})$")
        .expect("regex")
});
static ALL_HEX8: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9A-Fa-f]{8,}$").expect("regex"));
static GAME_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,4}(\.\d{1,10}){1,3}$").expect("regex"));
static GUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[0-9a-f]{8}[-_.]?[0-9a-f]{4}[-_.]?[0-9a-f]{4}[-_.]?[0-9a-f]{4}[-_.]?[0-9a-f]{12}",
    )
    .expect("regex")
});
static HANDLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)h[0-9a-f]{8}g[0-9a-f]{4}").expect("regex"));
static HEX32: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9A-Fa-f]{32,}").expect("regex"));
static DIGITS10: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]{10,}").expect("regex"));

/// The 19 BG3 `ItemSlot` names the contract allows (closed enum).
pub const EQUIPMENT_SLOTS: [&str; 19] = [
    "Helmet",
    "Breast",
    "Cloak",
    "MeleeMainHand",
    "MeleeOffHand",
    "RangedMainHand",
    "RangedOffHand",
    "Ring",
    "Underwear",
    "Boots",
    "Gloves",
    "Amulet",
    "Ring2",
    "Wings",
    "Horns",
    "Overhead",
    "MusicalInstrument",
    "VanityBody",
    "VanityBoots",
];

/// A GUID (any separator run, braced or not), a Larian handle, or ≥ 32 hex.
pub fn holds_guid(s: &str) -> bool {
    let stripped: String = s
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.' | '{' | '}'))
        .collect();
    GUID.is_match(s) || HANDLE.is_match(s) || HEX32.is_match(s) || HEX32.is_match(&stripped)
}

/// A run of 10+ decimal digits (SteamID64, Larian profile id).
pub fn holds_long_digits(s: &str) -> bool {
    DIGITS10.is_match(s)
}

fn leaky(s: &str) -> bool {
    holds_guid(s) || holds_long_digits(s)
}

/// An equipped item's stats id, or `None` (sent as `null`) when it does not
/// fit the contract shape — a modded item, a name, a path.
pub fn stats_id(raw: &str) -> Option<String> {
    (raw.len() <= 64 && STATS_ID.is_match(raw) && !leaky(raw)).then(|| raw.to_owned())
}

/// A Larian class / subclass id, or `None` when it cannot be one (a mod
/// class known only by UUID, a hex fragment, anything with a separator).
pub fn class_id(raw: &str) -> Option<String> {
    let ok = (1..=64).contains(&raw.len())
        && CLASS_ID.is_match(raw)
        && !ALL_HEX8.is_match(raw)
        && !leaky(raw);
    ok.then(|| raw.to_owned())
}

/// A NewAge component / enum type name, or `None` (the row is dropped).
pub fn type_name(raw: &str) -> Option<String> {
    let ok = raw.len() <= 128 && TYPE_NAME.is_match(raw) && !ALL_HEX8.is_match(raw) && !leaky(raw);
    ok.then(|| raw.to_owned())
}

/// `SaveInfo.json` "Game Version", or `None` when it does not fit.
pub fn game_version(raw: &str) -> Option<String> {
    (GAME_VERSION.is_match(raw) && !holds_long_digits(raw)).then(|| raw.to_owned())
}

pub fn equipment_slot(raw: &str) -> Option<&'static str> {
    EQUIPMENT_SLOTS.iter().copied().find(|s| *s == raw)
}

/// Last line of defence: `true` when no key or string anywhere in `body`
/// holds a GUID or a 10+ digit run (`saveRef` values are exempt from the
/// digit rule, as in the contract). A body that fails is never sent.
pub fn body_is_clean(body: &Value) -> bool {
    match body {
        Value::String(s) => !leaky(s),
        Value::Array(items) => items.iter().all(body_is_clean),
        Value::Object(map) => map.iter().all(|(k, v)| {
            let value_ok = match (k.as_str(), v) {
                ("saveRef", Value::String(s)) => !holds_guid(s),
                _ => body_is_clean(v),
            };
            !leaky(k) && value_ok
        }),
        _ => true,
    }
}
