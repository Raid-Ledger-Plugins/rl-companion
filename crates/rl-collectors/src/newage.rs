//! Read-only reader for a BG3 save's `NewAge` blob (`Globals.lsf`, magic
//! `LSMF`): the component table, the `core.v0.Level` owner maps, and
//! bounds-checked element access. No field decoding lives here.
//!
//! Ported from **bg3-newage** (`src/bg3_newage/lsmf.py`), MIT licence,
//! Copyright (c) 2026 Jessica Smith, <https://github.com/NodeJSmith/bg3-newage>;
//! full licence text in `THIRD_PARTY_NOTICES.md` at the repo root.
//! The layout is SOURCED from that project (reversed against one Patch 8
//! save), not yet VERIFIED against the operator's saves (ROK-2078 sweep):
//!
//! | Offset | Field |
//! |---|---|
//! | 0 | magic `LSMF`, u32 version, u64 hash |
//! | 16 | u64 block offset, u64 block size, u32 name-block size |
//! | 36 | u16 component count, u16 ?, u64 ? (header = 48 bytes) |
//!
//! **Every stored offset is relative to the end of the 48-byte header.** The
//! name block (packed UTF-8) sits at the block offset; the component table
//! follows it: `count × 48 B` entries of `u64 nameOffset (into the name
//! block), u32 nameSize, u32 ? (junk), u64 hash, u32 elementSize, u32
//! version, u64 elementCount, u64 dataOffset`. Component 0 must be
//! `core.v0.Level`, whose one element is `[entitiesStart, entitiesEnd)` of
//! 16-byte GUIDs and `[mapsStart, mapsEnd)` of owner maps (`u64 indicesStart,
//! u64 indicesEnd, u64 componentIndex, u64 count`, indices are u32 entity
//! indices). Element `k` of a component belongs to entity `owners[k]`; a
//! component with no owner map is a side table. An all-ones offset marks an
//! empty range.
//!
//! Hostile input: every read is bounds-checked with checked arithmetic, every
//! allocation is bounded by the blob length (owner maps: one per component,
//! `count ≤ entities`, `Σ count ≤ blob / 4`, reserved fallibly), and nothing
//! panics. Entity GUIDs
//! are counted, never kept; [`Lsmf::entity_guid`] reads one on demand
//! (ROK-2079 OQ-4 hashes it into a character key — it never leaves the PC).

use std::fmt;

pub const LSMF_MAGIC: [u8; 4] = *b"LSMF";
/// Largest blob the reader accepts (spike S2: blob ≤ 32 MiB; real ≈ 6.7 MB).
pub const LSMF_MAX_BYTES: usize = 32 * 1024 * 1024;
const HEADER: usize = 48;
const ENTRY: usize = 48;
const MAX_NAME: usize = 256;
const LEVEL: &str = "core.v0.Level";
const EMPTY: u64 = u64::MAX;

/// Why a blob is not a readable LSMF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LsmfError(pub &'static str);

impl fmt::Display for LsmfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NewAge blob is not readable: {}", self.0)
    }
}

impl std::error::Error for LsmfError {}

/// One component-table row; `owners` is `None` for a side table.
#[derive(Debug, Clone)]
pub struct LsmfComponent {
    pub name: String,
    pub version: u32,
    pub element_size: u32,
    pub element_count: u64,
    data_offset: u64,
    owners: Option<Vec<u32>>,
}

/// A parsed LSMF blob borrowing the bytes.
pub struct Lsmf<'a> {
    data: &'a [u8],
    components: Vec<LsmfComponent>,
    entity_count: u64,
    entities_start: u64,
}

fn rd(data: &[u8], offset: u64, len: u64) -> Result<&[u8], LsmfError> {
    let start = offset
        .checked_add(HEADER as u64)
        .ok_or(LsmfError("offset overflows"))?;
    let end = start
        .checked_add(len)
        .ok_or(LsmfError("length overflows"))?;
    let (start, end) = (usize::try_from(start), usize::try_from(end));
    match (start, end) {
        (Ok(s), Ok(e)) => data.get(s..e).ok_or(LsmfError("read is out of bounds")),
        _ => Err(LsmfError("read is out of bounds")),
    }
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
        .map_or(0, u64::from_le_bytes)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map_or(0, u32::from_le_bytes)
}

/// Number of `size`-byte records in `[start, end)`; `0` for an empty range.
fn records(data: &[u8], start: u64, end: u64, size: u64) -> Result<u64, LsmfError> {
    if start == EMPTY {
        return Ok(0);
    }
    if end < start || !(end - start).is_multiple_of(size) {
        return Err(LsmfError("range is not a whole number of records"));
    }
    rd(data, start, end - start)?;
    Ok((end - start) / size)
}

impl<'a> Lsmf<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, LsmfError> {
        if data.len() > LSMF_MAX_BYTES {
            return Err(LsmfError("blob is larger than the cap"));
        }
        if data.len() < HEADER || data[..4] != LSMF_MAGIC {
            return Err(LsmfError("bad magic"));
        }
        let block = u64_at(data, 16);
        let name_block = u64::from(u32_at(data, 32));
        let count = u64::from(u16::from_le_bytes([data[36], data[37]]));
        let table_at = block
            .checked_add(name_block)
            .ok_or(LsmfError("table offset overflows"))?;
        let table = rd(data, table_at, count * ENTRY as u64)?;
        let mut components = Vec::with_capacity(count as usize);
        for e in table.chunks_exact(ENTRY) {
            components.push(component(data, block, name_block, e)?);
        }
        if components.first().map(|c| c.name.as_str()) != Some(LEVEL) {
            return Err(LsmfError("component 0 is not core.v0.Level"));
        }
        let level = rd(data, components[0].data_offset, 32)?;
        let entity_count = records(data, u64_at(level, 0), u64_at(level, 8), 16)?;
        let mut me = Self {
            data,
            components,
            entity_count,
            entities_start: u64_at(level, 0),
        };
        me.owner_maps(u64_at(level, 16), u64_at(level, 24))?;
        Ok(me)
    }

    /// Every owner map, under three bounds that together cap the owners held
    /// at `blob / 4` u32s: one map per component, `count ≤ entity_count`, and
    /// a running total. Zero-size components pass the element bounds check at
    /// any count, and maps may share one indices range, so without these a
    /// ≤ 32 MiB blob could materialise gigabytes of owners (ROK-2078 review).
    fn owner_maps(&mut self, start: u64, end: u64) -> Result<(), LsmfError> {
        let n = records(self.data, start, end, 32)?;
        let budget = (self.data.len() / 4) as u64;
        let mut total = 0u64;
        for i in 0..n {
            let m = rd(self.data, start + i * 32, 32)?;
            let (indices, indices_end) = (u64_at(m, 0), u64_at(m, 8));
            let (comp, count) = (u64_at(m, 16), u64_at(m, 24));
            let Some(c) = usize::try_from(comp)
                .ok()
                .filter(|&c| c < self.components.len())
            else {
                return Err(LsmfError("owner map names a missing component"));
            };
            if self.components[c].owners.is_some() {
                return Err(LsmfError("component has two owner maps"));
            }
            if count != self.components[c].element_count {
                return Err(LsmfError("owner map size differs from the element count"));
            }
            if count > self.entity_count {
                return Err(LsmfError("owner map is larger than the entity table"));
            }
            total = total
                .checked_add(count)
                .filter(|&t| t <= budget)
                .ok_or(LsmfError("owner maps exceed the blob"))?;
            let owners = self.owners_at(indices, indices_end, count)?;
            self.components[c].owners = Some(owners);
        }
        Ok(())
    }

    /// `count` u32 owners at `[indices, indices_end)`, allocated fallibly.
    fn owners_at(&self, indices: u64, indices_end: u64, count: u64) -> Result<Vec<u32>, LsmfError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let bytes = count
            .checked_mul(4)
            .ok_or(LsmfError("owner map overflows"))?;
        if indices_end.checked_sub(indices) != Some(bytes) {
            return Err(LsmfError("owner map end does not match its count"));
        }
        let raw = rd(self.data, indices, bytes)?;
        let mut owners: Vec<u32> = Vec::new();
        owners
            .try_reserve_exact(raw.len() / 4)
            .map_err(|_| LsmfError("owner map allocation failed"))?;
        owners.extend(raw.chunks_exact(4).map(|b| u32_at(b, 0)));
        if owners.iter().any(|&o| u64::from(o) >= self.entity_count) {
            return Err(LsmfError("owner index is past the entity table"));
        }
        Ok(owners)
    }

    pub fn components(&self) -> &[LsmfComponent] {
        &self.components
    }

    pub fn entity_count(&self) -> u64 {
        self.entity_count
    }

    /// The raw 16 bytes of entity `index`'s GUID as stored (no byte-order
    /// conversion), read on demand and never kept; `None` past the table.
    pub fn entity_guid(&self, index: u32) -> Option<[u8; 16]> {
        if u64::from(index) >= self.entity_count {
            return None;
        }
        let at = u64::from(index)
            .checked_mul(16)?
            .checked_add(self.entities_start)?;
        <[u8; 16]>::try_from(rd(self.data, at, 16).ok()?).ok()
    }

    pub fn component(&self, name: &str) -> Option<&LsmfComponent> {
        self.components.iter().find(|c| c.name == name)
    }

    /// `(entity index, element bytes)` for each owned element of `name`;
    /// `None` when absent, a side table, or an entity owns two elements.
    pub fn by_entity(&self, name: &str) -> Option<Vec<(u32, &'a [u8])>> {
        let c = self.component(name)?;
        let owners = c.owners.as_ref()?;
        let size = u64::from(c.element_size);
        let mut out: Vec<(u32, &'a [u8])> = Vec::new();
        out.try_reserve_exact(owners.len()).ok()?;
        for (k, &owner) in owners.iter().enumerate() {
            let at = (k as u64)
                .checked_mul(size)
                .and_then(|o| o.checked_add(c.data_offset))?;
            out.push((owner, rd(self.data, at, size).ok()?));
        }
        out.sort_by_key(|(o, _)| *o);
        if out.windows(2).any(|w| w[0].0 == w[1].0) {
            return None;
        }
        Some(out)
    }
}

fn component(
    data: &[u8],
    block: u64,
    name_block: u64,
    e: &[u8],
) -> Result<LsmfComponent, LsmfError> {
    let (name_at, name_len) = (u64_at(e, 0), u64::from(u32_at(e, 8)));
    if name_len as usize > MAX_NAME || name_at.saturating_add(name_len) > name_block {
        return Err(LsmfError("component name is outside the name block"));
    }
    let off = block
        .checked_add(name_at)
        .ok_or(LsmfError("name offset overflows"))?;
    let name = std::str::from_utf8(rd(data, off, name_len)?)
        .map_err(|_| LsmfError("component name is not UTF-8"))?;
    let (element_size, element_count) = (u32_at(e, 24), u64_at(e, 32));
    let data_offset = u64_at(e, 40);
    if element_count > 0 {
        let bytes = element_count
            .checked_mul(u64::from(element_size))
            .ok_or(LsmfError("element array overflows"))?;
        rd(data, data_offset, bytes)?;
    }
    Ok(LsmfComponent {
        name: name.to_owned(),
        version: u32_at(e, 28),
        element_size,
        element_count,
        data_offset,
        owners: None,
    })
}

/// Embedded RIFF/WebP images (`RIFF????WEBP`), counted, capped at 1 000.
pub fn count_webp_images(data: &[u8]) -> u64 {
    let mut n = 0u64;
    let mut at = 0usize;
    while let Some(i) = data
        .get(at..)
        .and_then(|d| d.windows(4).position(|w| w == b"RIFF"))
    {
        let start = at + i;
        if data.get(start + 8..start + 12) == Some(b"WEBP") {
            n += 1;
            if n >= 1_000 {
                break;
            }
        }
        at = start + 4;
    }
    n
}
