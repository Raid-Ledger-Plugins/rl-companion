//! Larian stats `.txt` reader (`Armor.txt`, `Weapon.txt`, `Object.txt`):
//! stats id → root template GUID + rarity, with `using` inheritance.
//!
//! **UNVERIFIED layout (U5):** blocks of lines
//!
//! ```text
//! new entry "ARM_Hat"
//! type "Armor"
//! using "ARM_Base"
//! data "RootTemplate" "<guid>"
//! data "Rarity" "Rare"
//! ```
//!
//! Lines are trimmed, CRLF is accepted, blank and `//` lines are skipped,
//! and keywords the catalogue does not use are ignored. Any other `new …`
//! line (another block kind) closes the current entry. A malformed
//! `new entry` / `type` / `using` line is an error; a `data` line is read
//! key first and is strict only for the two keys the join needs (spec D4),
//! any other key's value is never parsed. A trailing `// comment` is
//! allowed after the quoted strings. An empty value counts as unset, so the
//! parent's value is inherited. Ids outside [`is_bg3_id`] skip their block;
//! a resolved rarity outside [`STATS_RARITIES`] becomes `None`.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::CatalogueError;

/// Most entries one stats file may hold.
pub const STATS_MAX_ENTRIES: usize = 100_000;
/// Longest stats id kept (spec §4, `BG3_ID_RE`); a longer entry is skipped.
pub const STATS_MAX_ID_BYTES: usize = 64;
/// Rarities the catalogue reports, in canonical spelling (matched ignoring
/// ASCII case). UNVERIFIED beyond `Common…Legendary` (spec U5); `Story` is
/// LSLib's quest-item rarity.
pub const STATS_RARITIES: &[&str] = &[
    "Common",
    "Uncommon",
    "Rare",
    "VeryRare",
    "Legendary",
    "Story",
];
/// Most `using` hops followed from an entry to its ancestors.
pub const USING_MAX_DEPTH: usize = 16;
/// The stop check runs once per this many lines (spec §4 deadline).
pub const STATS_CHECK_EVERY_LINES: usize = 10_000;
/// Entry types that are items.
pub const STATS_ITEM_TYPES: &[&str] = &["Armor", "Weapon", "Object"];
/// `data` key holding the root template GUID.
pub const STATS_KEY_ROOT_TEMPLATE: &str = "RootTemplate";
/// `data` key holding the rarity (`Common` … `Legendary`).
pub const STATS_KEY_RARITY: &str = "Rarity";

const FILE: &str = "stats txt";

/// One `new entry` block, unresolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatsEntry {
    pub id: String,
    pub kind: Option<String>,
    pub using: Option<String>,
    pub root_template: Option<String>,
    pub rarity: Option<String>,
}

/// An item entry with its `using` chain applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedItem {
    pub id: String,
    pub kind: String,
    pub root_template: Option<String>,
    pub rarity: Option<String>,
}

/// Parse a stats file with no deadline.
pub fn parse_stats(text: &str) -> Result<Vec<StatsEntry>, CatalogueError> {
    parse_stats_with(text, &mut || false)
}

/// Parse a stats file, calling `should_stop` every
/// [`STATS_CHECK_EVERY_LINES`] lines; `true` aborts with `Timeout`.
pub fn parse_stats_with(
    text: &str,
    should_stop: &mut dyn FnMut() -> bool,
) -> Result<Vec<StatsEntry>, CatalogueError> {
    let mut out = Vec::new();
    let mut current: Option<StatsEntry> = None;
    for (i, raw) in text.lines().enumerate() {
        if i > 0 && i % STATS_CHECK_EVERY_LINES == 0 && should_stop() {
            return Err(CatalogueError::Timeout);
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        if keyword == "new" {
            out.extend(current.take());
            current = new_entry(rest, i + 1)?;
            if out.len() + usize::from(current.is_some()) > STATS_MAX_ENTRIES {
                return Err(too_many(out.len() + 1));
            }
        } else if let Some(entry) = current.as_mut() {
            apply_line(entry, keyword, rest, i + 1)?;
        }
    }
    out.extend(current);
    Ok(out)
}

/// `new entry "ID"` → a fresh entry; other `new` kinds and over-long ids
/// → `None` (the block is skipped).
fn new_entry(rest: &str, line: usize) -> Result<Option<StatsEntry>, CatalogueError> {
    let rest = rest.trim_start();
    let Some(args) = rest.strip_prefix("entry") else {
        return Ok(None);
    };
    let [id] = quoted::<1>(args, line)?;
    if !is_bg3_id(id) {
        return Ok(None);
    }
    Ok(Some(StatsEntry {
        id: id.to_owned(),
        ..StatsEntry::default()
    }))
}

fn apply_line(
    entry: &mut StatsEntry,
    keyword: &str,
    rest: &str,
    line: usize,
) -> Result<(), CatalogueError> {
    match keyword {
        "type" => entry.kind = non_empty(quoted::<1>(rest, line)?[0]),
        "using" => entry.using = non_empty(quoted::<1>(rest, line)?[0]),
        "data" => {
            let (key, after) = quoted_prefix(rest, line)?;
            if key == STATS_KEY_ROOT_TEMPLATE {
                entry.root_template = non_empty(quoted::<1>(after, line)?[0]);
            } else if key == STATS_KEY_RARITY {
                entry.rarity = non_empty(quoted::<1>(after, line)?[0]);
            }
        }
        _ => {}
    }
    Ok(())
}

/// `BG3_ID_RE` (`^[A-Za-z0-9_]{1,64}$`, ROK-2079 / spec §4).
pub fn is_bg3_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= STATS_MAX_ID_BYTES
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// The canonical spelling of a known rarity.
fn known_rarity(r: &str) -> Option<String> {
    STATS_RARITIES
        .iter()
        .find(|k| k.eq_ignore_ascii_case(r))
        .map(|k| (*k).to_owned())
}

/// One leading double-quoted string and the text after it.
fn quoted_prefix(rest: &str, line: usize) -> Result<(&str, &str), CatalogueError> {
    let body = rest
        .trim_start()
        .strip_prefix('"')
        .ok_or_else(|| parse_err(line, "expected a quoted string"))?;
    let end = body
        .find('"')
        .ok_or_else(|| parse_err(line, "unterminated quoted string"))?;
    Ok((&body[..end], &body[end + 1..]))
}

/// Exactly `N` double-quoted strings separated by whitespace, optionally
/// followed by a `// comment`.
fn quoted<const N: usize>(rest: &str, line: usize) -> Result<[&str; N], CatalogueError> {
    let mut out = [""; N];
    let mut s = rest;
    for slot in &mut out {
        (*slot, s) = quoted_prefix(s, line)?;
    }
    let tail = s.trim();
    if !tail.is_empty() && !tail.starts_with("//") {
        return Err(parse_err(line, "unexpected text after the quoted strings"));
    }
    Ok(out)
}

/// Apply each entry's `using` chain (parent values fill the child's unset
/// fields, nearest ancestor first) and keep the item types. A missing
/// parent or a cycle stops the walk with what was found so far; so does
/// [`USING_MAX_DEPTH`]. When an id is defined twice the later one wins.
/// Output is sorted by id.
pub fn resolve_using(entries: &[StatsEntry]) -> Vec<ResolvedItem> {
    let by_id: HashMap<&str, &StatsEntry> = entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut out = BTreeMap::new();
    for entry in by_id.values() {
        if let Some(item) = resolve_one(entry, &by_id) {
            out.insert(item.id.clone(), item);
        }
    }
    out.into_values().collect()
}

fn resolve_one(entry: &StatsEntry, by_id: &HashMap<&str, &StatsEntry>) -> Option<ResolvedItem> {
    let mut merged = entry.clone();
    let mut seen = HashSet::from([entry.id.as_str()]);
    let mut cur = entry;
    for _ in 0..USING_MAX_DEPTH {
        let filled = merged.kind.is_some() && merged.root_template.is_some();
        if filled && merged.rarity.is_some() {
            break;
        }
        let Some(parent) = cur.using.as_deref().and_then(|p| by_id.get(p)) else {
            break;
        };
        if !seen.insert(parent.id.as_str()) {
            break;
        }
        fill(&mut merged.kind, &parent.kind);
        fill(&mut merged.root_template, &parent.root_template);
        fill(&mut merged.rarity, &parent.rarity);
        cur = parent;
    }
    let kind = merged.kind?;
    STATS_ITEM_TYPES
        .iter()
        .any(|t| t.eq_ignore_ascii_case(&kind))
        .then_some(ResolvedItem {
            id: merged.id,
            kind,
            root_template: merged.root_template,
            rarity: merged.rarity.as_deref().and_then(known_rarity),
        })
}

fn fill(slot: &mut Option<String>, from: &Option<String>) {
    if slot.is_none() {
        slot.clone_from(from);
    }
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

fn too_many(size: usize) -> CatalogueError {
    CatalogueError::TooLarge {
        what: "stats entry count",
        size: size as u64,
        limit: STATS_MAX_ENTRIES as u64,
    }
}

fn parse_err(at: usize, reason: &'static str) -> CatalogueError {
    CatalogueError::Parse {
        file: FILE,
        at,
        reason,
    }
}
