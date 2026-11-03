//! Minimal read-only LSF v7 reader (`meta.lsf` inside a BG3 save): find one
//! attribute by name and return its value as text. No general tree API.
//!
//! Layout, from LSLib `LSFReader`/`LSFCommon`, VERIFIED against a real BG3
//! save's `meta.lsf` (Patch 7+, 1 661 bytes) in ROK-2070 L2 with
//! `examples/meta_lsf_dump.rs`:
//!
//! | Offset | Field |
//! |---|---|
//! | 0 | magic `LSOF` |
//! | 4 | version u32 = 7 |
//! | 8 | EngineVersion u64 (real: `0x02040000000002bc`) |
//! | 16 | u32 pairs `(uncompressed, onDisk)`: Strings, Keys, Nodes, Attributes, Values |
//! | 56 | CompressionFlags u8 (real: `0x22`, LZ4), then three zero bytes |
//! | 60 | MetadataFormat u32 (real: `0` → V2 records; bit 0 → V3) |
//! | 64 | sections back to back; the real file has Strings, Nodes, Attributes, Values and an empty Keys (0/0), so the Keys position is moot; a non-empty Keys is assumed to follow Values |
//!
//! Every real section is compressed and occupies `onDisk` bytes; a section
//! with `onDisk == 0` (or method 0) is stored and occupies `uncompressed`.
//! The low nibble of CompressionFlags picks LZ4 (2) or zstd (3); zlib is
//! refused. Real LZ4: Strings is a raw **block**; Nodes, Attributes and
//! Values are **frames** (FLG `0x40`: linked blocks, no checksums, no content
//! size; BD `0x40`). Frame checksums are skipped, not verified.
//!
//! Strings: `u32 buckets` (real: 512), each `u16 count` then
//! `count × (u16 len, utf8)`; a name ref is `(bucket << 16) | index`. The table
//! holds node and attribute names only, never values. Nodes V2 (12 B): `name,
//! firstAttribute i32, parent i32` (never parsed here). Attributes V2 (12 B):
//! `name u32, typeAndLength u32 (type = low 6 bits, length = high 26), node
//! i32`; values are contiguous in attribute order (offsets cumulative). V3
//! (16 B: `name, typeAndLength, next i32, offset u32`) is kept for other
//! producers but is not what BG3 writes.
//!
//! `GameID` is a **FixedString** (type 22): 36 printable ASCII characters
//! plus a NUL, on the `MetaData` node under the root `MetaData` region; it is
//! the only attribute with that exact name (`SaveGameID` and `GameSessionID`
//! are different attributes). The UUID (type 31) display form below is
//! LSLib's: the real file's base-module UUID renders as the published
//! GustavX module UUID only under this byte order.

use crate::error::LsfError;
use crate::lspk::{lz4_can_hold, zstd_decode};

/// `LSOF` magic.
pub const LSF_MAGIC: [u8; 4] = *b"LSOF";
/// The only supported LSF version (BG3 saves, engine 4.8).
pub const LSF_VERSION: u32 = 7;
/// Cap on one decompressed section (spec D4).
pub const LSF_MAX_SECTION_BYTES: u64 = 1024 * 1024;
/// Cap on string-table entries, buckets and strings alike (spec D4).
pub const LSF_MAX_STRINGS: usize = 65_536;

const METADATA_START: usize = 16;
/// Metadata order of the `(uncompressed, onDisk)` pairs.
const SECTION_NAMES: [&str; 5] = ["strings", "keys", "nodes", "attributes", "values"];
const SECTIONS_START: usize = 64;
const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4D, 0x18];
const UUID_TYPE: u32 = 31;
const STRING_TYPES: [u32; 6] = [20, 21, 22, 23, 29, 30];

/// One section's `(uncompressed, onDisk)` sizes from the metadata block.
#[derive(Clone, Copy)]
struct Sizes {
    name: &'static str,
    size: u32,
    on_disk: u32,
}

/// Find the first attribute called `name` (any node) and return its value:
/// string types without their NUL terminator, a UUID in LSLib's display form.
/// `Ok(None)` when no attribute has that name.
pub fn read_lsf_attribute(bytes: &[u8], name: &str) -> Result<Option<String>, LsfError> {
    if let Some(m) = bytes.get(0..4)
        && m != LSF_MAGIC
    {
        return Err(LsfError::BadMagic([m[0], m[1], m[2], m[3]]));
    }
    let version = le_u32(take(bytes, 0, 8, "header")?, 4);
    if version != LSF_VERSION {
        return Err(LsfError::UnsupportedVersion(version));
    }
    let head = take(bytes, 0, SECTIONS_START, "metadata")?;
    let [strings, _keys, nodes, attrs, values] = std::array::from_fn(|i| Sizes {
        name: SECTION_NAMES[i],
        size: le_u32(head, METADATA_START + i * 8),
        on_disk: le_u32(head, METADATA_START + i * 8 + 4),
    });
    let flags = head[56];
    let v3 = le_u32(head, 60) & 1 != 0;
    let mut at = SECTIONS_START;
    let names = read_strings(&section(bytes, &mut at, strings, flags)?)?;
    skip(bytes, &mut at, nodes, flags)?;
    let attributes = section(bytes, &mut at, attrs, flags)?;
    let values = section(bytes, &mut at, values, flags)?;
    find_attribute(&names, &attributes, &values, v3, name)
}

/// Bytes a section occupies on disk (mirrors LSLib `Decompress`).
fn disk_len(s: Sizes, flags: u8) -> usize {
    if s.on_disk == 0 || flags & 0x0F == 0 {
        s.size as usize
    } else {
        s.on_disk as usize
    }
}

fn skip(bytes: &[u8], at: &mut usize, s: Sizes, flags: u8) -> Result<(), LsfError> {
    let len = disk_len(s, flags);
    take(bytes, *at, len, s.name)?;
    *at += len;
    Ok(())
}

/// Read and decompress one section, advancing `at` past it.
fn section(bytes: &[u8], at: &mut usize, s: Sizes, flags: u8) -> Result<Vec<u8>, LsfError> {
    let size = u64::from(s.size);
    if size > LSF_MAX_SECTION_BYTES {
        return Err(LsfError::SectionTooLarge {
            section: s.name,
            size,
            limit: LSF_MAX_SECTION_BYTES,
        });
    }
    let raw = take(bytes, *at, disk_len(s, flags), s.name)?;
    *at += raw.len();
    let expected = s.size as usize;
    let fail = |reason: String| LsfError::Decode {
        section: s.name,
        reason,
    };
    let out = match flags & 0x0F {
        _ if s.on_disk == 0 => raw.to_vec(),
        0 => raw.to_vec(),
        2 if raw.starts_with(&LZ4_FRAME_MAGIC) => lz4_frame(raw, expected).map_err(fail)?,
        2 if !lz4_can_hold(raw.len() as u64, size) => {
            return Err(fail(format!("{} bytes cannot hold {size}", raw.len())));
        }
        2 => lz4_flex::block::decompress(raw, expected).map_err(|e| fail(e.to_string()))?,
        3 => zstd_decode(raw, expected).map_err(fail)?,
        method => return Err(LsfError::UnsupportedCompression(method)),
    };
    if out.len() != expected {
        return Err(fail(format!(
            "decompressed to {} bytes, expected {expected}",
            out.len()
        )));
    }
    Ok(out)
}

/// Decode an LZ4 frame (lz4 frame format 1.6). Checksums are skipped, not
/// verified (no xxhash dependency); output is capped at `expected` bytes.
fn lz4_frame(raw: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let short = || "LZ4 frame is truncated".to_owned();
    let flg = *raw.get(4).ok_or_else(short)?;
    if flg >> 6 != 1 || flg & 0x01 != 0 {
        return Err(format!("unsupported LZ4 frame descriptor {flg:#04x}"));
    }
    let block_checksum = if flg & 0x10 != 0 { 4 } else { 0 };
    // magic 4 + FLG + BD + optional content size 8 + header checksum.
    let mut at = 7 + if flg & 0x08 != 0 { 8 } else { 0 };
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    loop {
        let word = le_u32(raw.get(at..at + 4).ok_or_else(short)?, 0);
        at += 4;
        if word == 0 {
            return Ok(out);
        }
        let len = (word & 0x7FFF_FFFF) as usize;
        let block = raw.get(at..at + len).ok_or_else(short)?;
        at += len + block_checksum;
        let room = expected - out.len();
        if word & 0x8000_0000 != 0 {
            if len > room {
                return Err(format!("LZ4 frame holds more than {expected} bytes"));
            }
            out.extend_from_slice(block);
            continue;
        }
        let mut buf = vec![0u8; room];
        let dict = &out[out.len().saturating_sub(65_536)..];
        let n = lz4_flex::block::decompress_into_with_dict(block, &mut buf, dict)
            .map_err(|e| e.to_string())?;
        out.extend_from_slice(&buf[..n]);
    }
}

/// Parse the string table into buckets of names.
fn read_strings(s: &[u8]) -> Result<Vec<Vec<String>>, LsfError> {
    let fail = |reason: &str| LsfError::Decode {
        section: "strings",
        reason: reason.to_owned(),
    };
    let short = || fail("string table is truncated");
    let buckets = le_u32(s.get(0..4).ok_or_else(short)?, 0) as usize;
    if buckets > LSF_MAX_STRINGS {
        return Err(fail("too many string buckets"));
    }
    let (mut at, mut total) = (4, 0usize);
    let mut names = Vec::with_capacity(buckets.min(1024));
    for _ in 0..buckets {
        let count = usize::from(le_u16(s.get(at..at + 2).ok_or_else(short)?, 0));
        at += 2;
        total += count;
        if total > LSF_MAX_STRINGS {
            return Err(fail("too many strings"));
        }
        let mut bucket = Vec::with_capacity(count);
        for _ in 0..count {
            let len = usize::from(le_u16(s.get(at..at + 2).ok_or_else(short)?, 0));
            let text = s.get(at + 2..at + 2 + len).ok_or_else(short)?;
            at += 2 + len;
            let text = std::str::from_utf8(text).map_err(|_| fail("a name is not UTF-8"))?;
            bucket.push(text.to_owned());
        }
        names.push(bucket);
    }
    Ok(names)
}

/// Scan the attribute table for `wanted` and decode its value.
fn find_attribute(
    names: &[Vec<String>],
    attrs: &[u8],
    values: &[u8],
    v3: bool,
    wanted: &str,
) -> Result<Option<String>, LsfError> {
    let record = if v3 { 16 } else { 12 };
    if !attrs.len().is_multiple_of(record) {
        return Err(LsfError::Decode {
            section: "attributes",
            reason: format!("{} bytes is not a whole number of records", attrs.len()),
        });
    }
    let mut cumulative = 0u64;
    for rec in attrs.chunks_exact(record) {
        let name_ref = le_u32(rec, 0);
        let type_and_len = le_u32(rec, 4);
        let len = u64::from(type_and_len >> 6);
        let offset = if v3 {
            u64::from(le_u32(rec, 12))
        } else {
            cumulative
        };
        cumulative += len;
        let name = names
            .get((name_ref >> 16) as usize)
            .and_then(|b| b.get((name_ref & 0xFFFF) as usize))
            .ok_or(LsfError::BadStringRef(name_ref))?;
        if name == wanted {
            let value = usize::try_from(offset)
                .ok()
                .and_then(|o| values.get(o..o.checked_add(len as usize)?));
            let value = value.ok_or_else(|| LsfError::Decode {
                section: "values",
                reason: format!("{wanted} value {offset}+{len} is outside the values section"),
            })?;
            return decode_value(type_and_len & 0x3F, value, wanted).map(Some);
        }
    }
    Ok(None)
}

fn decode_value(type_id: u32, value: &[u8], wanted: &str) -> Result<String, LsfError> {
    let fail = |reason: String| LsfError::Decode {
        section: "values",
        reason,
    };
    if STRING_TYPES.contains(&type_id) {
        let end = value.iter().position(|&b| b == 0).unwrap_or(value.len());
        let text = std::str::from_utf8(&value[..end])
            .map_err(|_| fail(format!("{wanted} is not UTF-8")))?;
        return Ok(text.to_owned());
    }
    match <[u8; 16]>::try_from(value) {
        Ok(uuid) if type_id == UUID_TYPE => Ok(uuid_display(&uuid)),
        _ => Err(fail(format!(
            "{wanted} has type {type_id} and {} bytes; expected a string or a 16-byte UUID",
            value.len()
        ))),
    }
}

/// LSLib's UUID text: .NET `Guid` order (first three groups little-endian)
/// after `ByteSwapGuid` swaps each byte pair of the last eight bytes.
fn uuid_display(b: &[u8; 16]) -> String {
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 9, 8, 11, 10, 13, 12, 15, 14];
    let mut out = String::with_capacity(36);
    for (i, &at) in order.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push_str(&format!("{:02x}", b[at]));
    }
    out
}

/// Bounds-checked `bytes[start..start + len]`.
fn take<'a>(
    bytes: &'a [u8],
    start: usize,
    len: usize,
    what: &'static str,
) -> Result<&'a [u8], LsfError> {
    start
        .checked_add(len)
        .and_then(|end| bytes.get(start..end))
        .ok_or(LsfError::Truncated {
            what,
            need: start as u64 + len as u64,
            have: bytes.len() as u64,
        })
}

// Only called on slices whose length was checked.
fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
