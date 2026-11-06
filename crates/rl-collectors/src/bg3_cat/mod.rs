//! BG3 name catalogue (ROK-2081): the pure parsers behind the companion's
//! "stats id → item name / rarity" and "origin / quest id → name" tables.
//!
//! The crate never touches the filesystem (spec D1): the app discovers the
//! install, opens the paks with [`crate::PakSet`], keeps only the entries
//! [`classify_entry`] accepts (spec D3), and hands their bytes to
//! [`parse_loca_wanted`], [`parse_stats`] / [`resolve_using`] and
//! [`read_lsx_nodes`]. The join (stats → template → handle → text) and
//! stats-only rarity are [`CatalogueBuilder`] / [`CatalogueJoin`] (L3);
//! item names wait on ROK-2080's LSF tree walker (see `build.rs`).
//!
//! **UNVERIFIED layouts.** Every entry path below and the node / attribute
//! names in [`LSX_ORIGINS`] / [`LSX_QUESTS`] come from the spec's U5–U8 and
//! LSLib conventions, not from a probe of a real install. They are data, not
//! code: when the dev-bridge probe (spec §6 step 0) confirms or corrects
//! them, only these tables change. Matching ignores ASCII case and accepts
//! `\` separators, so a casing or separator difference cannot hide an entry.

mod build;
mod display;
mod loca;
mod lsx;
mod lsx_tag;
mod stats_txt;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use build::{
    CAT_MAX_MATCHED_ENTRIES, CAT_MAX_ROWS, CatalogueBuilder, CatalogueCounts, CatalogueJoin,
};
pub use display::{
    CAT_GUID_HEX_RUN, CAT_MAX_DIGIT_RUN, CAT_MAX_NAME_UTF16, catalogue_display_text,
};
pub use loca::{
    LOCA_ENTRY_BYTES, LOCA_HEADER_BYTES, LOCA_KEY_BYTES, LOCA_MAGIC, LOCA_MAX_ENTRIES,
    is_loca_handle, parse_loca_wanted,
};
pub use lsx::{
    LSX_MAX_ATTRS, LSX_MAX_DEPTH, LSX_MAX_NODES, LSX_MAX_RETAINED_BYTES, LSX_MAX_TAG_ATTRS,
    LSX_MAX_TOTAL_ATTRS, LsxAttr, LsxNode, LsxSource, lsx_id_handles, read_lsx_nodes,
};
pub use stats_txt::{
    ResolvedItem, STATS_CHECK_EVERY_LINES, STATS_ITEM_TYPES, STATS_KEY_RARITY,
    STATS_KEY_ROOT_TEMPLATE, STATS_MAX_ENTRIES, STATS_MAX_ID_BYTES, STATS_MAX_VALUE_BYTES,
    STATS_RARITIES, StatsEntry, USING_MAX_DEPTH, is_bg3_id, parse_stats, parse_stats_with,
    resolve_using,
};

/// Which parser a matched pak entry feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// `english.loca` (U4).
    Loca,
    /// A stats `.txt` with item entries (U5).
    StatsTxt,
    /// RootTemplates `_merged.lsf` (U6; parsed by L3's `templates.rs`).
    RootTemplates,
    /// `Origins.lsx` (U7).
    Origins,
    /// `quest_prototypes.lsx` (U8).
    Quests,
}

/// Entry paths the catalogue reads, one `/`-separated segment pattern per
/// path: `*` matches exactly one whole segment. UNVERIFIED (U4–U8).
pub const ENTRY_GLOBS: &[(&str, EntryKind)] = &[
    ("Localization/English/english.loca", EntryKind::Loca),
    (
        "Public/*/Stats/Generated/Data/Armor.txt",
        EntryKind::StatsTxt,
    ),
    (
        "Public/*/Stats/Generated/Data/Weapon.txt",
        EntryKind::StatsTxt,
    ),
    (
        "Public/*/Stats/Generated/Data/Object.txt",
        EntryKind::StatsTxt,
    ),
    (
        "Public/*/RootTemplates/_merged.lsf",
        EntryKind::RootTemplates,
    ),
    ("Public/*/Origins/Origins.lsx", EntryKind::Origins),
    (
        "Mods/*/Story/Journal/quest_prototypes.lsx",
        EntryKind::Quests,
    ),
];

/// Origins: node `Origin`, keyed by `Name`, named by `DisplayName` (U7).
pub const LSX_ORIGINS: LsxSource = LsxSource {
    node_id: "Origin",
    key_attr: "Name",
    handle_attr: "DisplayName",
};

/// Quests: node `Quest`, keyed by `QuestID`, named by `QuestTitle` (U8; the
/// node id is a guess until the probe confirms it).
pub const LSX_QUESTS: LsxSource = LsxSource {
    node_id: "Quest",
    key_attr: "QuestID",
    handle_attr: "QuestTitle",
};

/// The parser for a pak entry path, or `None` to skip the entry unread.
pub fn classify_entry(path: &str) -> Option<EntryKind> {
    let path = path.replace('\\', "/");
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    ENTRY_GLOBS
        .iter()
        .find(|(glob, _)| glob_matches(glob, &segs))
        .map(|&(_, kind)| kind)
}

fn glob_matches(glob: &str, segs: &[&str]) -> bool {
    let pats: Vec<&str> = glob.split('/').collect();
    pats.len() == segs.len()
        && pats
            .iter()
            .zip(segs)
            .all(|(p, s)| (*p == "*" && !s.is_empty()) || p.eq_ignore_ascii_case(s))
}

/// An item's display name and rarity; either may be unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemName {
    pub name: Option<String>,
    pub rarity: Option<String>,
}

/// The finished catalogue: only the final rows, never the loca map (D4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueTables {
    pub items: BTreeMap<String, ItemName>,
    pub origins: BTreeMap<String, String>,
    pub quests: BTreeMap<String, String>,
    pub waypoints: BTreeMap<String, String>,
}

impl CatalogueTables {
    /// The name and rarity for a stats id.
    pub fn lookup_item(&self, stats_id: &str) -> Option<&ItemName> {
        self.items.get(stats_id)
    }

    /// The display name for an origin `Name`.
    pub fn lookup_origin(&self, id: &str) -> Option<&str> {
        self.origins.get(id).map(String::as_str)
    }

    /// The title for a quest id.
    pub fn lookup_quest(&self, id: &str) -> Option<&str> {
        self.quests.get(id).map(String::as_str)
    }

    /// The name for a waypoint id (the table may ship empty, ruling 11).
    pub fn lookup_waypoint(&self, id: &str) -> Option<&str> {
        self.waypoints.get(id).map(String::as_str)
    }

    /// Total rows across every table.
    pub fn row_count(&self) -> usize {
        self.items.len() + self.origins.len() + self.quests.len() + self.waypoints.len()
    }
}
