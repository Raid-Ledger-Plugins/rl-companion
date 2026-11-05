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
//! The allowlist pins `(type, version, elementSize)` where a version is
//! known: [`Layout::version`] `None` means "not pinned yet", because upstream
//! records no real version values (it is never invented). The sweep reads the
//! versions it saw from `types[].version` and pins them here.
//!
//! **Party order (UNVERIFIED).** Entity-index order of the `PlayerComponent`
//! owners is not proven to be `SaveInfo` order (upstream joins on total XP
//! through an experience component whose layout is not ported, and XP totals
//! can tie), so `partyMatchesSaveInfo` is `true` only for a one-member party,
//! where the slot correspondence is trivial, and only then is `party[0]`
//! filled. A 2+ member party reports `decoded: {}` for every slot until the
//! order is proven (or the contract says `decoded` is in entity order). Class levels and equipment need game
//! data / the LSF `Items` tree and stay undecoded (`decoder: none`).

use super::bg3_character_key::EntityGuid;
use super::bg3_newage::{
    Checks, DecoderStatus, NewAgeDecoder, NewAgeOutcome, NewAgeRead, PartyDecoded, Readability,
    TypeRow,
};
use rl_collectors::{
    count_webp_images, lsf_attribute_len, read_lsf_attribute_bytes, Lsmf, LSMF_MAX_BYTES,
};

pub const NEWAGE_ATTRIBUTE: &str = "NewAge";
const STATS: &str = "game.stats.v3.StatsComponent";
const HEALTH: &str = "game.stats.v0.HealthComponent";
const PLAYER: &str = "game.v0.PlayerComponent";

/// One decoded layout; `None` = not pinned (any value is accepted).
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub name: &'static str,
    pub version: Option<u32>,
    pub element_size: Option<u32>,
}

/// The layouts this reader decodes. No version is pinned yet (see the module
/// doc); the PlayerComponent marker is any size.
pub const DECODED: [Layout; 3] = [
    Layout {
        name: STATS,
        version: None,
        element_size: Some(36),
    },
    Layout {
        name: HEALTH,
        version: None,
        element_size: Some(32),
    },
    Layout {
        name: PLAYER,
        version: None,
        element_size: None,
    },
];

/// The LSMF-backed decoder wired into `DevBridge::set_bg3_source`.
pub struct LsmfDecoder;

/// Why the `NewAge` bytes were not returned.
#[derive(Debug, PartialEq, Eq)]
enum BlobError {
    /// The LSF tables or values cannot be read → `lsf-unreadable`.
    Lsf,
    /// The LSF reads but the blob is over [`LSMF_MAX_BYTES`] → `newage-unreadable`.
    OverCap,
}

/// The `NewAge` bytes: `Ok(None)` when absent. The size is checked from the
/// attribute table BEFORE the values section is decompressed.
fn blob(globals_lsf: &[u8]) -> Result<Option<Vec<u8>>, BlobError> {
    let len = lsf_attribute_len(globals_lsf, NEWAGE_ATTRIBUTE).map_err(|_| BlobError::Lsf)?;
    if len.is_some_and(|n| n > LSMF_MAX_BYTES as u64) {
        return Err(BlobError::OverCap);
    }
    let bytes =
        read_lsf_attribute_bytes(globals_lsf, NEWAGE_ATTRIBUTE).map_err(|_| BlobError::Lsf)?;
    if bytes.as_ref().is_some_and(|b| b.len() > LSMF_MAX_BYTES) {
        return Err(BlobError::OverCap);
    }
    Ok(bytes)
}

impl NewAgeDecoder for LsmfDecoder {
    fn read(&self, globals_lsf: &[u8], party_size: usize) -> NewAgeOutcome {
        let Ok(Some(bytes)) = blob(globals_lsf) else {
            return NewAgeOutcome::Unreadable;
        };
        match Lsmf::parse(&bytes) {
            Ok(l) => NewAgeOutcome::Read(decode(&l, &bytes, party_size)),
            Err(_) => NewAgeOutcome::Unreadable,
        }
    }

    /// `LsfUnreadable` when the values cannot be read, so the listing never
    /// advertises a full read the structure route lacks; an over-cap blob is
    /// `NotLsmf` (the LSF read fine; the blob is what cannot be read).
    fn readable(&self, globals_lsf: &[u8]) -> Readability {
        match blob(globals_lsf) {
            Ok(Some(b)) if Lsmf::parse(&b).is_ok() => Readability::Lsmf,
            Ok(Some(_)) | Err(BlobError::OverCap) => Readability::NotLsmf,
            Ok(None) => Readability::NotChecked,
            Err(BlobError::Lsf) => Readability::LsfUnreadable,
        }
    }
}

pub fn status(name: &str, version: u32, element_size: u32) -> DecoderStatus {
    status_in(&DECODED, name, version, element_size)
}

/// `Ok` only when every pinned field of the layout matches the triple.
pub fn status_in(table: &[Layout], name: &str, version: u32, element_size: u32) -> DecoderStatus {
    let Some(l) = table.iter().find(|l| l.name == name) else {
        return DecoderStatus::None;
    };
    let pinned = |want: Option<u32>, got: u32| want.is_none_or(|w| w == got);
    if pinned(l.version, version) && pinned(l.element_size, element_size) {
        DecoderStatus::Ok
    } else {
        DecoderStatus::Unsupported
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
    let ok = l
        .component(name)
        .is_some_and(|c| status(name, c.version, c.element_size) == DecoderStatus::Ok);
    if !ok {
        return Vec::new();
    }
    l.by_entity(name).unwrap_or_default()
}

fn find<'a>(rows: &[(u32, &'a [u8])], entity: u32) -> Option<&'a [u8]> {
    rows.binary_search_by_key(&entity, |r| r.0)
        .ok()
        .map(|i| rows[i].1)
}

fn member(stats: Option<&[u8]>, health: Option<&[u8]>, guid: Option<[u8; 16]>) -> PartyDecoded {
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
        entity_guid: guid.map(EntityGuid),
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
                decoder: status(&c.name, c.version, c.element_size),
            })
        })
        .collect()
}

fn decode(l: &Lsmf, bytes: &[u8], party_size: usize) -> NewAgeRead {
    let party: Vec<u32> = decodable(l, PLAYER).into_iter().map(|r| r.0).collect();
    // The correspondence is proven only for one member (see the module doc).
    // `party[k]` means SaveInfo slot k, so rows are filled only when it is
    // proven; otherwise `decoded` stays `{}` rather than risk naming the
    // wrong character (review MINOR).
    let proven = party_size == 1 && party.len() == 1;
    let checks = Checks {
        party_entities_found: party.len() as u64,
        party_matches_save_info: proven,
        portraits_found: count_webp_images(bytes),
        ..Checks::default()
    };
    let (stats, health) = (decodable(l, STATS), decodable(l, HEALTH));
    let members = if proven {
        party
            .iter()
            .map(|&e| member(find(&stats, e), find(&health, e), l.entity_guid(e)))
            .collect()
    } else {
        Vec::new()
    };
    NewAgeRead {
        types: types(l),
        checks,
        party: members,
    }
}
