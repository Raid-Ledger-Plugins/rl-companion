//! `GET /dev/v1/bg3/saves` and `/dev/v1/bg3/saves/<saveRef>/structure`
//! (dev-bridge/v1, ROK-2077 contract; ROK-2078 layer 1). Every body is
//! ≤ 16 KiB, paged by row with the BG3 cursor `p<page>_<section>_<offset>`,
//! and passes [`body_is_clean`] before it is sent.

use super::bg3_newage::{checks_json, party_row, type_row, Checks, NewAgeOutcome};
use super::bg3_redact::body_is_clean;
use super::bg3_saves::Bg3Source;
use super::paging::{invalid, json_len, Query, PAGE_MAX_BYTES};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::{LazyLock, Mutex};

type Res = Result<Value, (u16, Value)>;

static CURSOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^p([1-9][0-9]{0,3})_(saves|types|party)_([0-9]{1,6})$").expect("regex")
});
static SAVE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{12}$").expect("regex"));
/// The longest cursor the grammar allows; pages reserve room for it.
const WORST_NEXT: &str = "p9999_types_999999";
/// One BG3 read at a time: a request may hold a 256 MiB `.lsv` plus up to
/// 64 MiB of decompressed sections, so concurrent connections queue here
/// rather than multiply that (ROK-2078 review).
static READS: Mutex<()> = Mutex::new(());
/// Saves opened per listing page (a row is ≤ ~450 bytes, so 32 fit 16 KiB).
pub const SAVES_PER_PAGE: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub page: usize,
    pub section: String,
    pub offset: usize,
}

pub fn encode(c: &Cursor) -> String {
    format!("p{}_{}_{}", c.page, c.section, c.offset)
}

pub fn decode(raw: &str) -> Option<Cursor> {
    let m = CURSOR.captures(raw)?;
    Some(Cursor {
        page: m.get(1)?.as_str().parse().ok()?,
        section: m.get(2)?.as_str().to_owned(),
        offset: m.get(3)?.as_str().parse().ok()?,
    })
}

fn not_found() -> (u16, Value) {
    (404, json!({ "error": "not_found" }))
}

fn cursor_arg(q: &Query) -> Result<Option<Cursor>, (u16, Value)> {
    match q.get("cursor") {
        None => Ok(None),
        Some(raw) => decode(raw).map(Some).ok_or_else(|| (400, invalid("cursor", "Unknown cursor"))),
    }
}

/// Route a `/dev/v1/bg3/*` GET; `None` when the path is not one of ours.
pub fn get(src: Option<&Bg3Source>, path: &str, q: &Query) -> Option<(u16, Value)> {
    let rest = path.strip_prefix("/dev/v1/bg3/saves")?;
    let _one_at_a_time = src.map(|_| READS.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    if src.is_some_and(|s| s.key().is_none()) {
        return Some((500, json!({ "error": "internal" })));
    }
    let res = match (src, rest) {
        (None, "" | "/") => Ok(json!({ "saves": [], "next": null })),
        (None, _) => Err(not_found()),
        (Some(s), "") => saves_ep(s, q),
        (Some(s), r) => {
            let save_ref = r.strip_prefix('/')?.strip_suffix("/structure")?;
            if !SAVE_REF.is_match(save_ref) {
                return Some(not_found());
            }
            structure_ep(s, save_ref, q)
        }
    };
    Some(match res.and_then(guard) {
        Ok(body) => (200, body),
        Err(e) => e,
    })
}

/// Refuse (500, never send) a body that would leak or exceed 16 KiB.
fn guard(body: Value) -> Res {
    if body_is_clean(&body) && json_len(&body) <= PAGE_MAX_BYTES {
        Ok(body)
    } else {
        Err((500, json!({ "error": "internal" })))
    }
}

fn saves_ep(src: &Bg3Source, q: &Query) -> Res {
    let files = src.save_files();
    let (page, offset) = match cursor_arg(q)? {
        None => (1, 0),
        Some(c) if c.section == "saves" && c.offset == (c.page - 1) * SAVES_PER_PAGE && c.offset < files.len() => {
            (c.page, c.offset)
        }
        Some(_) => return Err(not_found()),
    };
    let end = (offset + SAVES_PER_PAGE).min(files.len());
    let rows: Vec<Value> = files[offset..end]
        .iter()
        .filter_map(|(abs, rel)| src.open(abs, rel).map(|o| o.row))
        .collect();
    let next = (end < files.len()).then(|| encode(&Cursor { page: page + 1, section: "saves".into(), offset: end }));
    Ok(json!({ "saves": rows, "next": next }))
}

fn structure_ep(src: &Bg3Source, save_ref: &str, q: &Query) -> Res {
    let want = cursor_arg(q)?;
    let (abs, _) = src.find(save_ref).ok_or_else(not_found)?;
    let opened = src.open_structure(&abs).ok_or_else(not_found)?;
    let outcome = match &opened.globals {
        Some(g) => src.decoder.read(g, opened.party_size),
        None => NewAgeOutcome::NotDecoded,
    };
    let read = match outcome {
        NewAgeOutcome::Read(r) => Some(r),
        NewAgeOutcome::NotDecoded | NewAgeOutcome::Unreadable => None,
    };
    let checks = checks_json(read.as_ref().map_or(&Checks::default(), |r| &r.checks));
    let mut items: Vec<(&'static str, Value)> = Vec::new();
    if let Some(r) = &read {
        items.extend(r.types.iter().filter_map(type_row).map(|v| ("types", v)));
    }
    for slot in 0..opened.party_size.min(16) {
        let decoded = read.as_ref().and_then(|r| r.party.get(slot));
        items.push(("party", party_row(slot, decoded)));
    }
    let head = |types: Vec<Value>, party: Vec<Value>, next: Value| {
        json!({ "saveRef": save_ref, "types": types, "checks": checks, "party": party, "next": next })
    };
    let pages = split(&items, json_len(&head(vec![], vec![], json!(WORST_NEXT))));
    let index = match want {
        None => 0,
        Some(c) => page_index(&pages, &items, &c).ok_or_else(not_found)?,
    };
    let (start, end) = pages.get(index).copied().unwrap_or((0, 0));
    let slice = items.get(start..end).unwrap_or_default();
    let pick = |s: &str| slice.iter().filter(|(k, _)| *k == s).map(|(_, v)| v.clone()).collect();
    let next = pages.get(index + 1).map(|&(s, _)| encode(&cursor_at(&items, index + 2, s)));
    Ok(head(pick("types"), pick("party"), json!(next)))
}

/// The cursor naming item `at` as the first row of page `page`.
fn cursor_at(items: &[(&'static str, Value)], page: usize, at: usize) -> Cursor {
    let section = items.get(at).map_or("types", |(k, _)| *k);
    let offset = items[..at].iter().filter(|(k, _)| *k == section).count();
    Cursor { page, section: section.into(), offset }
}

/// The page a cursor names, only if recomputing the pages lands exactly on
/// it — a save rewritten since the first page gets 404 and restarts.
fn page_index(pages: &[(usize, usize)], items: &[(&'static str, Value)], c: &Cursor) -> Option<usize> {
    let index = c.page.checked_sub(1)?;
    let &(start, _) = pages.get(index)?;
    (index > 0 && cursor_at(items, c.page, start) == *c).then_some(index)
}

/// Greedy row pages: types first, then party, never splitting a row; at most
/// 512 types and 16 party rows per page, every page ≤ 16 KiB with `head`.
fn split(items: &[(&'static str, Value)], head: usize) -> Vec<(usize, usize)> {
    let mut pages = Vec::new();
    let mut start = 0;
    loop {
        let (mut used, mut end, mut types, mut party) = (head, start, 0usize, 0usize);
        while let Some((kind, v)) = items.get(end) {
            let add = json_len(v) + 1;
            let (t, p) = if *kind == "types" { (types + 1, party) } else { (types, party + 1) };
            if used + add > PAGE_MAX_BYTES || t > 512 || p > 16 {
                break;
            }
            (used, types, party, end) = (used + add, t, p, end + 1);
        }
        if end == start && start < items.len() {
            end += 1;
        }
        pages.push((start, end));
        if end >= items.len() {
            return pages;
        }
        start = end;
    }
}
