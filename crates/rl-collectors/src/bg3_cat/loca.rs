//! `.loca` localisation reader: handle → English text (ruling 5), keeping
//! only the handles the caller asks for (spec D5, two-pass loca).
//!
//! **UNVERIFIED layout (U4, LSLib `LocaReader`):** a 12-byte header
//! (`LOCA` magic, `u32` entry count, `u32` offset of the text block), then
//! `count` × 70-byte entries (`key[64]` NUL-padded UTF-8, `u16` version,
//! `u32` text length), then the texts back to back from the text offset,
//! each `length` bytes including one trailing NUL. All little-endian.
//!
//! A text that is not UTF-8, or a key that is not UTF-8, drops only that
//! entry. A header, entry table or text running past the input is an error.

use std::collections::{HashMap, HashSet};

use crate::error::CatalogueError;
use crate::lspk::le_u32;

/// The four magic bytes at the start of a `.loca` file.
pub const LOCA_MAGIC: [u8; 4] = *b"LOCA";
/// Header size: magic, entry count, text-block offset.
pub const LOCA_HEADER_BYTES: usize = 12;
/// Size of the NUL-padded key field in each entry.
pub const LOCA_KEY_BYTES: usize = 64;
/// Size of one entry: key, `u16` version, `u32` length.
pub const LOCA_ENTRY_BYTES: usize = LOCA_KEY_BYTES + 2 + 4;
/// Most entries a `.loca` may declare (English BG3 holds a few hundred
/// thousand, U12 — UNVERIFIED; the cap only bounds the table walk).
pub const LOCA_MAX_ENTRIES: u32 = 2 * 1024 * 1024;

const FILE: &str = "english.loca";

/// True for a BG3 TranslatedString handle: `h` + 8 hex + `g` + 4 hex +
/// `g` + 4 hex + `g` + 4 hex + `g` + 12 hex (37 chars), lowercase or
/// uppercase hex.
pub fn is_loca_handle(s: &str) -> bool {
    let b = s.as_bytes();
    let groups = [8usize, 4, 4, 4, 12];
    if b.len() != 37 || b[0] != b'h' {
        return false;
    }
    let mut at = 1;
    for (i, &n) in groups.iter().enumerate() {
        if i > 0 {
            if b[at] != b'g' {
                return false;
            }
            at += 1;
        }
        if !b[at..at + n].iter().all(u8::is_ascii_hexdigit) {
            return false;
        }
        at += n;
    }
    true
}

/// Texts for the `wanted` handles that the file holds. Handles not in the
/// file are simply absent from the map. Later duplicates of a key win.
pub fn parse_loca_wanted(
    bytes: &[u8],
    wanted: &HashSet<String>,
) -> Result<HashMap<String, String>, CatalogueError> {
    let (count, texts_at) = read_header(bytes)?;
    let mut out = HashMap::new();
    let mut text_at = texts_at;
    for i in 0..count {
        let entry = LOCA_HEADER_BYTES + i * LOCA_ENTRY_BYTES;
        let len = le_u32(bytes, entry + LOCA_KEY_BYTES + 2) as usize;
        let end = text_at
            .checked_add(len)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| parse_err(text_at, "text runs past the end of the file"))?;
        if let Some(key) = entry_key(&bytes[entry..entry + LOCA_KEY_BYTES])
            && wanted.contains(key)
            && let Some(text) = entry_text(&bytes[text_at..end])
        {
            out.insert(key.to_owned(), text);
        }
        text_at = end;
    }
    Ok(out)
}

/// `(entry count, text-block offset)`, with the entry table bounds-checked.
fn read_header(bytes: &[u8]) -> Result<(usize, usize), CatalogueError> {
    if bytes.len() < LOCA_HEADER_BYTES {
        return Err(parse_err(0, "header truncated"));
    }
    if bytes[..4] != LOCA_MAGIC {
        return Err(parse_err(0, "not a LOCA file"));
    }
    let count = le_u32(bytes, 4);
    if count > LOCA_MAX_ENTRIES {
        return Err(CatalogueError::TooLarge {
            what: "loca entry count",
            size: u64::from(count),
            limit: u64::from(LOCA_MAX_ENTRIES),
        });
    }
    let count = count as usize;
    let table_end = LOCA_HEADER_BYTES + count * LOCA_ENTRY_BYTES;
    if table_end > bytes.len() {
        return Err(parse_err(
            LOCA_HEADER_BYTES,
            "entry table runs past the end",
        ));
    }
    let texts_at = le_u32(bytes, 8) as usize;
    if texts_at < table_end || texts_at > bytes.len() {
        return Err(parse_err(8, "text offset outside the file"));
    }
    Ok((count, texts_at))
}

/// The key up to its first NUL, or `None` if empty or not UTF-8.
fn entry_key(field: &[u8]) -> Option<&str> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..end])
        .ok()
        .filter(|k| !k.is_empty())
}

/// The text without its one trailing NUL, or `None` if not UTF-8.
fn entry_text(raw: &[u8]) -> Option<String> {
    let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
    String::from_utf8(raw.to_vec()).ok()
}

fn parse_err(at: usize, reason: &'static str) -> CatalogueError {
    CatalogueError::Parse {
        file: FILE,
        at,
        reason,
    }
}
