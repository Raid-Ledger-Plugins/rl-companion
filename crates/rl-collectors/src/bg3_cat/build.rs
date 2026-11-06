//! The catalogue join (spec D4/D5, L3): stats → root template → loca
//! handle → text for items, LSX key → loca handle → text for origins and
//! quests, and stats-only rarity.
//!
//! Two passes, so the loca map is never held whole (D5):
//!
//! 1. [`CatalogueBuilder`] takes each matched entry's bytes in turn —
//!    [`add_stats`](CatalogueBuilder::add_stats) for stats `.txt`,
//!    [`add_lsx`](CatalogueBuilder::add_lsx) for `Origins.lsx` /
//!    `quest_prototypes.lsx`, and
//!    [`add_template_handles`](CatalogueBuilder::add_template_handles) for
//!    root-template `MapKey → DisplayName` pairs — then
//!    [`into_join`](CatalogueBuilder::into_join) applies `using` across all
//!    stats files and works out which handles are needed.
//! 2. The caller scans `english.loca` with
//!    [`parse_loca_wanted`](super::parse_loca_wanted) for
//!    [`CatalogueJoin::wanted_handles`] and hands the texts to
//!    [`CatalogueJoin::finish`], which applies the D9 display filter.
//!
//! **Rarity is stats-only:** an item's rarity is its own or its nearest
//! `using` ancestor's `data "Rarity"`, canonicalised to
//! [`STATS_RARITIES`](super::STATS_RARITIES); it never depends on a
//! template or a loca string, so it is known even when the name is not.
//!
//! **Item names — `TODO(ROK-2080)`:** the `MapKey → DisplayName` pairs come
//! from RootTemplates `_merged.lsf`, which needs ROK-2080's LSF tree walker
//! (`lsf.rs` reads single attributes only). Until that lands nothing calls
//! [`CatalogueBuilder::add_template_handles`] in the app, so every item's
//! `name` is `None` and only its rarity is known (spec §9.2 fallback).
//! `templates.rs` (spec §3) plugs in there and nowhere else.
//!
//! A later duplicate key wins: the caller feeds entries in pak priority
//! order, and duplicates are dropped by key *before* any text is resolved,
//! so a later copy whose text is missing or refused — or an LSX copy with
//! no usable display-name handle at all — still hides the earlier one. A key that could not be a payload object key
//! ([`catalogue_key_ok`]: not a `BG3_ID_RE` id, or one holding a GUID shape
//! or a 10+ digit run) is dropped and counted in `skipped`. Waypoints have no source entry yet (ruling 11), so that table
//! stays empty.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::display::{catalogue_display_text, catalogue_key_ok};
use super::lsx::lsx_id_entries;
use super::{
    CatalogueTables, EntryKind, ItemName, LSX_ORIGINS, LSX_QUESTS, StatsEntry, is_loca_handle,
    parse_stats_with, read_lsx_nodes, resolve_using,
};
use crate::error::CatalogueError;

/// Most matched pak entries one build may feed in (spec §4).
pub const CAT_MAX_MATCHED_ENTRIES: usize = 4_096;
/// Most rows any one source may retain across a build — stats entries,
/// LSX ids, template pairs — and so most final rows per table (spec §4).
pub const CAT_MAX_ROWS: usize = 200_000;

/// Row counts of a finished build (the cache's `counts`, spec §5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CatalogueCounts {
    /// Item rows (a name or a rarity is known).
    pub items: usize,
    /// Item rows whose name is known.
    pub items_named: usize,
    pub origins: usize,
    pub quests: usize,
    pub waypoints: usize,
    /// Wanted handles with no loca text, a text the D9 filter refused, an
    /// origin / quest key with no usable handle, or a key that is not a
    /// safe payload key.
    pub skipped: usize,
}

/// Pass 1: collects stats entries, LSX ids and template handles.
#[derive(Debug, Default)]
pub struct CatalogueBuilder {
    entries: usize,
    stats: Vec<StatsEntry>,
    origins: Vec<(String, Option<String>)>,
    quests: Vec<(String, Option<String>)>,
    templates: HashMap<String, String>,
    refused_keys: usize,
}

impl CatalogueBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// One stats `.txt`; `should_stop` is the deadline check.
    pub fn add_stats(
        &mut self,
        text: &str,
        should_stop: &mut dyn FnMut() -> bool,
    ) -> Result<(), CatalogueError> {
        self.count_entry()?;
        let parsed = parse_stats_with(text, should_stop)?;
        check_rows("stats entries", self.stats.len() + parsed.len())?;
        self.stats.extend(parsed);
        Ok(())
    }

    /// One `Origins.lsx` or `quest_prototypes.lsx`; any other kind is a
    /// caller bug and refused.
    pub fn add_lsx(&mut self, kind: EntryKind, text: &str) -> Result<(), CatalogueError> {
        let src = match kind {
            EntryKind::Origins => &LSX_ORIGINS,
            EntryKind::Quests => &LSX_QUESTS,
            _ => return Err(not_lsx()),
        };
        self.count_entry()?;
        let (pairs, refused): (Vec<_>, Vec<_>) =
            lsx_id_entries(&read_lsx_nodes(text, src.node_id)?, src)
                .into_iter()
                .partition(|(key, _)| catalogue_key_ok(key));
        let rows = match kind {
            EntryKind::Origins => &mut self.origins,
            _ => &mut self.quests,
        };
        check_rows("LSX ids", rows.len() + pairs.len())?;
        rows.extend(pairs);
        self.refused_keys += refused.len();
        Ok(())
    }

    /// Root-template `(MapKey GUID, DisplayName handle)` pairs from one
    /// entry. `TODO(ROK-2080)`: fed by `templates.rs` once the LSF tree
    /// walker exists. GUIDs match the stats `RootTemplate` ignoring ASCII
    /// case; a pair whose handle is not a loca handle is dropped.
    pub fn add_template_handles(
        &mut self,
        pairs: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(), CatalogueError> {
        self.count_entry()?;
        for (guid, handle) in pairs {
            if guid.is_empty() || !is_loca_handle(&handle) {
                continue;
            }
            self.templates.insert(guid.to_ascii_lowercase(), handle);
            check_rows("template handles", self.templates.len())?;
        }
        Ok(())
    }

    /// Resolve `using` across every stats file and list the needed handles.
    /// Duplicates are dropped by key (last wins) here, before any text.
    pub fn into_join(self) -> CatalogueJoin {
        let mut refused_keys = self.refused_keys;
        let mut items = BTreeMap::new();
        for it in resolve_using(&self.stats) {
            if !catalogue_key_ok(&it.id) {
                refused_keys += 1;
                continue;
            }
            let handle = it
                .root_template
                .and_then(|rt| self.templates.get(&rt.to_ascii_lowercase()).cloned());
            let rarity = it.rarity;
            items.insert(it.id, JoinItem { handle, rarity });
        }
        let origins: BTreeMap<_, _> = self.origins.into_iter().collect();
        let quests: BTreeMap<_, _> = self.quests.into_iter().collect();
        let wanted = items
            .values()
            .filter_map(|it| it.handle.clone())
            .chain(origins.values().flatten().cloned())
            .chain(quests.values().flatten().cloned())
            .collect();
        CatalogueJoin {
            items,
            origins,
            quests,
            wanted,
            refused_keys,
        }
    }

    fn count_entry(&mut self) -> Result<(), CatalogueError> {
        self.entries = counted(self.entries)?;
        Ok(())
    }
}

#[derive(Debug)]
struct JoinItem {
    handle: Option<String>,
    rarity: Option<String>,
}

/// Pass 2: the needed handles, then the finished tables.
#[derive(Debug)]
pub struct CatalogueJoin {
    /// Keyed, so already deduplicated (last wins).
    items: BTreeMap<String, JoinItem>,
    /// `None`: the winning copy has no usable handle, so no name.
    origins: BTreeMap<String, Option<String>>,
    quests: BTreeMap<String, Option<String>>,
    wanted: HashSet<String>,
    refused_keys: usize,
}

impl CatalogueJoin {
    /// Every loca handle the tables need; pass to `parse_loca_wanted`.
    pub fn wanted_handles(&self) -> &HashSet<String> {
        &self.wanted
    }

    /// Join the loca texts. An item row is kept when its name or its
    /// rarity is known; an origin / quest row only when its text survives
    /// the D9 filter.
    pub fn finish(self, texts: &HashMap<String, String>) -> (CatalogueTables, CatalogueCounts) {
        let mut counts = CatalogueCounts {
            skipped: self.refused_keys,
            ..CatalogueCounts::default()
        };
        let mut name_of = |handle: Option<&str>| {
            let name = handle
                .and_then(|h| texts.get(h))
                .and_then(|t| catalogue_display_text(t));
            counts.skipped += usize::from(name.is_none());
            name
        };
        let mut items = BTreeMap::new();
        for (id, it) in self.items {
            // An item with no handle is not a skip: its rarity may stand.
            let name = it.handle.as_deref().and_then(|h| name_of(Some(h)));
            if name.is_some() || it.rarity.is_some() {
                let row = ItemName {
                    name,
                    rarity: it.rarity,
                };
                items.insert(id, row);
            }
        }
        let origins = named(self.origins, &mut name_of);
        let quests = named(self.quests, &mut name_of);
        counts.items = items.len();
        counts.items_named = items.values().filter(|r| r.name.is_some()).count();
        counts.origins = origins.len();
        counts.quests = quests.len();
        let tables = CatalogueTables {
            items,
            origins,
            quests,
            waypoints: BTreeMap::new(),
        };
        (tables, counts)
    }
}

fn named(
    rows: BTreeMap<String, Option<String>>,
    name_of: &mut impl FnMut(Option<&str>) -> Option<String>,
) -> BTreeMap<String, String> {
    rows.into_iter()
        .filter_map(|(key, handle)| Some((key, name_of(handle.as_deref())?)))
        .collect()
}

fn counted(entries: usize) -> Result<usize, CatalogueError> {
    let next = entries + 1;
    if next > CAT_MAX_MATCHED_ENTRIES {
        return Err(CatalogueError::TooLarge {
            what: "matched entry count",
            size: next as u64,
            limit: CAT_MAX_MATCHED_ENTRIES as u64,
        });
    }
    Ok(next)
}

fn check_rows(what: &'static str, size: usize) -> Result<(), CatalogueError> {
    if size > CAT_MAX_ROWS {
        return Err(CatalogueError::TooLarge {
            what,
            size: size as u64,
            limit: CAT_MAX_ROWS as u64,
        });
    }
    Ok(())
}

fn not_lsx() -> CatalogueError {
    CatalogueError::Parse {
        file: "catalogue build",
        at: 0,
        reason: "entry kind is not an LSX id table",
    }
}
