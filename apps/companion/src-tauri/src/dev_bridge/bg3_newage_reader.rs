//! ROK-2078 layer 2: the real [`NewAgeDecoder`] over `Globals.lsf` →
//! `NewAge` (LSMF, parsed by `rl_collectors::Lsmf`).
//!
//! Field layouts are ported from **bg3-newage** (`src/bg3_newage/party.py`),
//! MIT licence, Copyright (c) 2026 Jessica Smith,
//! <https://github.com/NodeJSmith/bg3-newage> (full licence text:
//! `THIRD_PARTY_NOTICES.md`). SOURCED (one Patch 8 save),
//! not yet VERIFIED on the operator's saves:
//!
//! - `game.stats.v3.StatsComponent` (36 B): 8 unknown bytes, six i32 ability
//!   scores STR..CHA, i32 proficiency bonus.
//! - `game.stats.v0.HealthComponent` (32 B): i32 current HP, i32 max HP, 24
//!   unknown bytes.
//! - `game.v0.PlayerComponent`: a marker; its owners are the active party.
//!
//! The allowlist pins `(type, elementSize)`; the version column is reported
//! but not pinned, because upstream does not record the real version values.
//! Party rows map to `SaveInfo` slots in ascending entity-index order, and
//! only when the party count matches `SaveInfo` (upstream joins on total XP
//! instead; the order is UNVERIFIED). Class levels and equipment need game
//! data / the LSF `Items` tree and stay undecoded (`decoder: none`).

use super::bg3_newage::{
    Checks, DecoderStatus, NewAgeDecoder, NewAgeOutcome, NewAgeRead, PartyDecoded, Readability, TypeRow,
};
use rl_collectors::{count_webp_images, lsf_attribute_len, read_lsf_attribute_bytes, Lsmf, LSMF_MAX_BYTES};

pub const NEWAGE_ATTRIBUTE: &str = "NewAge";
const STATS: &str = "game.stats.v3.StatsComponent";
const HEALTH: &str = "game.stats.v0.HealthComponent";
const PLAYER: &str = "game.v0.PlayerComponent";
/// `(type, elementSize)` pairs this reader decodes; `None` = any size (marker).
const DECODED: [(&str, Option<u32>); 3] = [(STATS, Some(36)), (HEALTH, Some(32)), (PLAYER, None)];

/// The LSMF-backed decoder wired into `DevBridge::set_bg3_source`.
pub struct LsmfDecoder;

/// The `NewAge` bytes: `Ok(None)` when absent, `Err` when the LSF values
/// cannot be read or the blob is over the cap. The size is checked from the
/// attribute table BEFORE the values section is decompressed.
fn blob(globals_lsf: &[u8]) -> Result<Option<Vec<u8>>, ()> {
    let len = lsf_attribute_len(globals_lsf, NEWAGE_ATTRIBUTE).map_err(|_| ())?;
    if len.is_some_and(|n| n > LSMF_MAX_BYTES as u64) {
        return Err(());
    }
    let bytes = read_lsf_attribute_bytes(globals_lsf, NEWAGE_ATTRIBUTE).map_err(|_| ())?;
    if bytes.as_ref().is_some_and(|b| b.len() > LSMF_MAX_BYTES) {
        return Err(());
    }
    Ok(bytes)
}

impl NewAgeDecoder for LsmfDecoder {
    fn read(&self, globals_lsf: &[u8], party_size: usize) -> NewAgeOutcome {
        let Ok(Some(bytes)) = blob(globals_lsf) else { return NewAgeOutcome::Unreadable };
        match Lsmf::parse(&bytes) {
            Ok(l) => NewAgeOutcome::Read(decode(&l, &bytes, party_size)),
            Err(_) => NewAgeOutcome::Unreadable,
        }
    }

    /// `LsfUnreadable` when the values (or an over-cap blob) cannot be read,
    /// so the listing never advertises a full read the structure route lacks.
    fn readable(&self, globals_lsf: &[u8]) -> Readability {
        match blob(globals_lsf) {
            Ok(Some(b)) if Lsmf::parse(&b).is_ok() => Readability::Lsmf,
            Ok(Some(_)) => Readability::NotLsmf,
            Ok(None) => Readability::NotChecked,
            Err(()) => Readability::LsfUnreadable,
        }
    }
}

pub fn status(name: &str, element_size: u32) -> DecoderStatus {
    match DECODED.iter().find(|(n, _)| *n == name) {
        Some((_, Some(size))) if *size != element_size => DecoderStatus::Unsupported,
        Some(_) => DecoderStatus::Ok,
        None => DecoderStatus::None,
    }
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    i32_at(b, at).and_then(|v| u32::try_from(v).ok())
}

/// Elements of `name` by entity, only when its layout is on the allowlist.
fn decodable<'a>(l: &Lsmf<'a>, name: &str) -> Vec<(u32, &'a [u8])> {
    let ok = l.component(name).is_some_and(|c| status(name, c.element_size) == DecoderStatus::Ok);
    if !ok {
        return Vec::new();
    }
    l.by_entity(name).unwrap_or_default()
}

fn find<'a>(rows: &[(u32, &'a [u8])], entity: u32) -> Option<&'a [u8]> {
    rows.binary_search_by_key(&entity, |r| r.0).ok().map(|i| rows[i].1)
}

fn member(stats: Option<&[u8]>, health: Option<&[u8]>) -> PartyDecoded {
    let abilities = stats.and_then(|s| {
        let v: Vec<u32> = (0..6).filter_map(|k| u32_at(s, 8 + k * 4)).collect();
        <[u32; 6]>::try_from(v).ok()
    });
    PartyDecoded {
        abilities,
        proficiency_bonus: stats.and_then(|s| u32_at(s, 32)),
        max_hp: health.and_then(|h| u32_at(h, 4)),
        class_levels: None,
        equipment: None,
    }
}

fn types(l: &Lsmf) -> Vec<TypeRow> {
    l.components()
        .iter()
        .filter_map(|c| {
            Some(TypeRow {
                type_name: c.name.clone(),
                version: u16::try_from(c.version).ok()?,
                element_size: c.element_size,
                count: c.element_count,
                decoder: status(&c.name, c.element_size),
            })
        })
        .collect()
}

fn decode(l: &Lsmf, bytes: &[u8], party_size: usize) -> NewAgeRead {
    let party: Vec<u32> = decodable(l, PLAYER).into_iter().map(|r| r.0).collect();
    let matches = party_size > 0 && party.len() == party_size;
    let checks = Checks {
        party_entities_found: party.len() as u64,
        party_matches_save_info: matches,
        portraits_found: count_webp_images(bytes),
        ..Checks::default()
    };
    let (stats, health) = (decodable(l, STATS), decodable(l, HEALTH));
    let members = if matches {
        party.iter().map(|&e| member(find(&stats, e), find(&health, e))).collect()
    } else {
        Vec::new()
    };
    NewAgeRead { types: types(l), checks, party: members }
}
