//! THE SEAM lane B fills (ROK-2078 layer 2): what a NewAge (LSMF) reader
//! hands the `bg3-save-structure` body. Layer 1 ships [`NotYetDecoded`],
//! which reports "not yet decoded" in the contract's allowed form: an empty
//! `types` table, zero/false `checks`, and `{ slot, decoded: {} }` party rows.
//!
//! A decoder returns raw values; [`party_row`] / [`type_row`] apply the
//! contract's redaction (drop, never rewrite), so a decoder cannot leak by
//! shape. It must still emit only values it read from game data.

use super::bg3_character_key::{character_key_prefix, EntityGuid};
use super::bg3_redact::{class_id, equipment_slot, stats_id, type_name};
use serde_json::{json, Map, Value};

/// Per-type decode status (`types[].decoder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderStatus {
    /// This exact `(type, version, elementSize)` triple is decoded.
    Ok,
    /// A decoder exists but the triple differs (layout drift).
    Unsupported,
    /// No decoder for this type.
    None,
}

/// One NewAge type-table row. Counts only, never element bytes.
#[derive(Debug, Clone)]
pub struct TypeRow {
    pub type_name: String,
    pub version: u16,
    pub element_size: u32,
    pub count: u64,
    pub decoder: DecoderStatus,
}

/// The S0 verification answers (`checks`), as counts and booleans.
#[derive(Debug, Clone, Default)]
pub struct Checks {
    pub party_entities_found: u64,
    pub party_matches_save_info: bool,
    pub portraits_found: u64,
    pub portraits_mapped: u64,
    pub meta_names_match_party: bool,
    pub owner_profile_matches_local: bool,
}

#[derive(Debug, Clone)]
pub struct ClassLevel {
    pub class_id: String,
    pub subclass_id: Option<String>,
    pub level: u32,
}

/// One party member's decoded fields; `None` = its decoder is unsupported/none.
#[derive(Debug, Clone, Default)]
pub struct PartyDecoded {
    /// `str dex con int wis cha`.
    pub abilities: Option<[u32; 6]>,
    pub proficiency_bonus: Option<u32>,
    pub max_hp: Option<u32>,
    pub class_levels: Option<Vec<ClassLevel>>,
    /// `(ItemSlot name, stats id)`.
    pub equipment: Option<Vec<(String, String)>>,
    /// The member's entity GUID (ROK-2079 OQ-4). Only its hash prefix is
    /// ever sent; `None` = the GUID could not be read.
    pub entity_guid: Option<EntityGuid>,
}

/// What one save's NewAge read produced.
#[derive(Debug, Clone, Default)]
pub struct NewAgeRead {
    pub types: Vec<TypeRow>,
    pub checks: Checks,
    /// By 0-based party index (`SaveInfo` order); missing entries send `{}`.
    pub party: Vec<PartyDecoded>,
}

pub enum NewAgeOutcome {
    /// No reader yet (layer 1), or this save was not attempted.
    NotDecoded,
    /// The blob is not a readable LSMF (`fallbackReason: newage-unreadable`).
    Unreadable,
    Read(NewAgeRead),
}

/// Lane B implements this over the `Globals.lsf` bytes (≤ 64 MiB) of one save.
pub trait NewAgeDecoder: Send + Sync {
    fn read(&self, globals_lsf: &[u8], party_size: usize) -> NewAgeOutcome;
    /// LSMF check for the `bg3-saves` listing (`fallbackReason`).
    fn readable(&self, _globals_lsf: &[u8]) -> Readability {
        Readability::NotChecked
    }
}

/// What the listing's NewAge check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readability {
    /// No check made (layer 1): no fallback is reported for it.
    NotChecked,
    /// The blob parsed as LSMF.
    Lsmf,
    /// The bytes were extracted but are not LSMF → `newage-unreadable`.
    NotLsmf,
    /// The LSF values could not be read (corrupt, truncated, over the cap)
    /// → `lsf-unreadable`, the same reason the structure read implies.
    LsfUnreadable,
}

/// Layer 1's decoder: nothing is decoded yet.
pub struct NotYetDecoded;

impl NewAgeDecoder for NotYetDecoded {
    fn read(&self, _globals_lsf: &[u8], _party_size: usize) -> NewAgeOutcome {
        NewAgeOutcome::NotDecoded
    }
}

pub fn checks_json(c: &Checks) -> Value {
    let cap = |n: u64| n.min(100_000_000);
    json!({
        "partyEntitiesFound": cap(c.party_entities_found),
        "partyMatchesSaveInfo": c.party_matches_save_info,
        "portraitsFound": cap(c.portraits_found),
        "portraitsMapped": cap(c.portraits_mapped),
        "metaNamesMatchParty": c.meta_names_match_party,
        "ownerProfileMatchesLocal": c.owner_profile_matches_local,
    })
}

/// A `types[]` row, or `None` when the type name or a number does not fit.
pub fn type_row(r: &TypeRow) -> Option<Value> {
    let ty = type_name(&r.type_name)?;
    if r.element_size > 1_048_576 || r.count > 100_000_000 {
        return None;
    }
    let decoder = match r.decoder {
        DecoderStatus::Ok => "ok",
        DecoderStatus::Unsupported => "unsupported",
        DecoderStatus::None => "none",
    };
    Some(
        json!({ "type": ty, "version": r.version, "elementSize": r.element_size,
        "count": r.count, "decoder": decoder }),
    )
}

fn in_range(v: u32, lo: u32, hi: u32) -> Option<u32> {
    (lo..=hi).contains(&v).then_some(v)
}

/// The `decoded` object: each field present only when it fits the contract.
pub fn decoded_json(d: &PartyDecoded) -> Value {
    let mut out = Map::new();
    if let Some(a) = d
        .abilities
        .filter(|a| a.iter().all(|v| (1..=30).contains(v)))
    {
        let [s, dx, c, i, w, ch] = a;
        out.insert(
            "abilities".into(),
            json!({"str": s, "dex": dx, "con": c, "int": i, "wis": w, "cha": ch}),
        );
    }
    if let Some(p) = d.proficiency_bonus.and_then(|p| in_range(p, 1, 10)) {
        out.insert("proficiencyBonus".into(), p.into());
    }
    if let Some(hp) = d.max_hp.and_then(|h| in_range(h, 1, 100_000)) {
        out.insert("maxHp".into(), hp.into());
    }
    if let Some(levels) = &d.class_levels {
        out.insert("classLevels".into(), Value::Array(class_levels(levels)));
    }
    if let Some(items) = &d.equipment {
        out.insert("equipment".into(), Value::Array(equipment(items)));
    }
    Value::Object(out)
}

/// An unmapped class (`classId` not a Larian id) is left out entirely.
fn class_levels(levels: &[ClassLevel]) -> Vec<Value> {
    levels
        .iter()
        .filter_map(|c| {
            let id = class_id(&c.class_id)?;
            let level = in_range(c.level, 1, 20)?;
            let sub = c.subclass_id.as_deref().and_then(class_id);
            Some(json!({ "classId": id, "subclassId": sub, "level": level }))
        })
        .take(12)
        .collect()
}

/// Unknown slots are dropped, a slot appears once, a modded stats id is `null`.
fn equipment(items: &[(String, String)]) -> Vec<Value> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for (slot, id) in items {
        let Some(slot) = equipment_slot(slot) else {
            continue;
        };
        if seen.contains(&slot) {
            continue;
        }
        seen.push(slot);
        out.push(json!({ "slot": slot, "statsId": stats_id(id) }));
    }
    out
}

/// One `party[]` row; `decoded` is `{}` when nothing was decoded for it.
/// `characterKeyPrefix` is present only when both the member's entity GUID
/// and the save's 64-hex `campaign_key` are known; the GUID never is.
pub fn party_row(slot: usize, d: Option<&PartyDecoded>, campaign_key: Option<&str>) -> Value {
    let decoded = d.map_or_else(|| json!({}), decoded_json);
    let mut row = json!({ "slot": slot, "decoded": decoded });
    let prefix = d
        .and_then(|d| d.entity_guid.as_ref())
        .and_then(|g| character_key_prefix(campaign_key, g));
    if let Some(p) = prefix {
        row["characterKeyPrefix"] = Value::String(p);
    }
    row
}
