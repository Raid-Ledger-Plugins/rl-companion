//! Multi-part LSPK v18 reader over `Read + Seek` (ROK-2081: BG3 game paks).
//!
//! Game paks are multi-GB and may be split into parts, so unlike
//! [`crate::read_lspk`] this never holds a whole package: it reads the
//! header and file table from part 0, then seeks to one entry at a time in
//! `parts[entry.archive_part]`. Part *n* (n ≥ 1) of `X.pak` is `X_n.pak` in
//! the same folder, and an entry's offset is relative to its own part
//! (LSLib convention, UNVERIFIED on a real install — spec ROK-2081 U3).
//!
//! The crate never touches the filesystem: the caller opens the part files
//! and hands over the readers (any `Read + Seek`, e.g. `File` or `Cursor`).
//!
//! Every read is bounded before anything is allocated: the part count, the
//! file table (`MAX_FILES`), each entry ([`PAK_MAX_ENTRY_BYTES`] or the
//! caller's tighter limit), each entry's on-disk bytes (its decompressed
//! size plus LZ4's worst-case overhead), and the total decompressed bytes
//! read through one set ([`PAK_MAX_SET_BYTES`]).

use std::io::{Read, Seek, SeekFrom};

use crate::error::LspkError;
use crate::lspk::{
    DEFAULT_MAX_ENTRY_BYTES, ENTRY_LEN, Entry, HEADER_LEN, LSPK_MAGIC, LSPK_VERSION, MAX_FILES,
    decode, decode_file_table, le_u16, le_u32, le_u64,
};

/// Most archive parts one set may open, or its header may declare.
pub const PAK_MAX_PARTS: usize = 16;
/// Default cap on one decompressed entry (spec ROK-2081 §4, ruling 3).
pub const PAK_MAX_ENTRY_BYTES: u64 = DEFAULT_MAX_ENTRY_BYTES;
/// Default cap on the decompressed bytes read through one set (§4).
pub const PAK_MAX_SET_BYTES: u64 = 512 * 1024 * 1024;
/// Fixed slack a compressed entry may carry on top of LZ4's worst-case
/// `size / 255` expansion (block/frame headers).
pub const PAK_RAW_SLACK_BYTES: u64 = 64;

/// The fixed 40-byte LSPK v18 header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PakHeader {
    pub version: u32,
    pub file_list_offset: u64,
    pub flags: u8,
    pub priority: u8,
    /// `NumParts` as written; 0 is read as 1.
    pub num_parts: u16,
}

/// The file name of part `part` of the pak named `base`: part 0 is `base`
/// itself, part n is `<stem>_n.<ext>` (`Gustav.pak` → `Gustav_1.pak`).
pub fn part_file_name(base: &str, part: u8) -> String {
    if part == 0 {
        return base.to_owned();
    }
    match base.rfind('.') {
        Some(dot) if dot > 0 => format!("{}_{part}{}", &base[..dot], &base[dot..]),
        _ => format!("{base}_{part}"),
    }
}

/// Read and check the header at the start of `r` (part 0 of a pak). Lets
/// the caller learn [`PakHeader::num_parts`] before opening sibling parts.
pub fn read_pak_header<R: Read + Seek>(r: &mut R) -> Result<PakHeader, LspkError> {
    let len = stream_len(r)?;
    let h = read_range(r, len, 0, HEADER_LEN as u64, "header")?;
    if h[0..4] != LSPK_MAGIC {
        return Err(LspkError::BadMagic([h[0], h[1], h[2], h[3]]));
    }
    let version = le_u32(&h, 4);
    if version != LSPK_VERSION {
        return Err(LspkError::UnsupportedVersion(version));
    }
    let num_parts = le_u16(&h, 38).max(1);
    if usize::from(num_parts) > PAK_MAX_PARTS {
        return Err(part_count(usize::from(num_parts)));
    }
    Ok(PakHeader {
        version,
        file_list_offset: le_u64(&h, 8),
        flags: h[20],
        priority: h[21],
        num_parts,
    })
}

struct Part<R> {
    reader: R,
    len: u64,
}

/// An opened pak: part 0's header and file table, plus every part reader.
pub struct PakSet<R> {
    parts: Vec<Part<R>>,
    header: PakHeader,
    entries: Vec<Entry>,
    used: u64,
    set_limit: u64,
}

impl<R: Read + Seek> PakSet<R> {
    /// Open a set. `parts[0]` is the base `.pak`; `parts[n]` is part n.
    /// Fewer parts than the header declares is allowed: entries in a part
    /// that was not opened fail with [`LspkError::MissingArchivePart`].
    pub fn open(readers: Vec<R>) -> Result<Self, LspkError> {
        if readers.is_empty() || readers.len() > PAK_MAX_PARTS {
            return Err(part_count(readers.len()));
        }
        let mut parts = Vec::with_capacity(readers.len());
        for mut reader in readers {
            let len = stream_len(&mut reader)?;
            parts.push(Part { reader, len });
        }
        let base = &mut parts[0];
        let header = read_pak_header(&mut base.reader)?;
        let entries = read_table(base, header.file_list_offset)?;
        Ok(Self {
            parts,
            header,
            entries,
            used: 0,
            set_limit: PAK_MAX_SET_BYTES,
        })
    }

    /// Replace the set's total byte budget (tighten only in production).
    pub fn with_set_limit(mut self, limit: u64) -> Self {
        self.set_limit = limit;
        self
    }

    pub fn header(&self) -> PakHeader {
        self.header
    }

    /// All file-table entries, in table order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many parts were opened.
    pub fn parts_opened(&self) -> usize {
        self.parts.len()
    }

    /// Decompressed bytes read through this set so far.
    pub fn bytes_read(&self) -> u64 {
        self.used
    }

    /// Read and decompress one entry, refusing anything over `limit`, over
    /// the set budget, or in a part that was not opened — before allocating.
    pub fn read_entry_limited(&mut self, entry: &Entry, limit: u64) -> Result<Vec<u8>, LspkError> {
        let size = entry.size();
        let name = || entry.name.clone();
        if size > limit {
            return Err(LspkError::EntryTooLarge {
                name: name(),
                size,
                limit,
            });
        }
        if !within_budget(self.used, size, self.set_limit) {
            return Err(LspkError::SetBudgetExceeded {
                name: name(),
                size,
                used: self.used,
                limit: self.set_limit,
            });
        }
        let on_disk = u64::from(entry.size_on_disk);
        if on_disk > raw_cap(size) {
            return Err(LspkError::RawTooLarge {
                name: name(),
                size_on_disk: on_disk,
                size,
            });
        }
        let opened = self.parts.len();
        let part = self
            .parts
            .get_mut(usize::from(entry.archive_part))
            .ok_or_else(|| LspkError::MissingArchivePart {
                name: name(),
                part: entry.archive_part,
                opened,
            })?;
        let raw = read_range(
            &mut part.reader,
            part.len,
            entry.offset,
            on_disk,
            "entry data",
        )?;
        let expected = usize::try_from(size).map_err(|_| LspkError::EntryTooLarge {
            name: name(),
            size,
            limit,
        })?;
        let out = decode(entry, &raw, expected)?;
        if out.len() != expected {
            return Err(LspkError::SizeMismatch {
                name: name(),
                expected: size,
                actual: out.len() as u64,
            });
        }
        self.used += size;
        Ok(out)
    }
}

/// Whether reading `next` more bytes keeps `used` within `limit`.
pub(crate) fn within_budget(used: u64, next: u64, limit: u64) -> bool {
    used.checked_add(next).is_some_and(|total| total <= limit)
}

/// The most on-disk bytes an entry of `size` decompressed bytes can need:
/// LZ4's worst case (`size + size / 255 + 16`) plus header slack. A zstd
/// frame of raw blocks costs 3 bytes per 128 KiB, well inside it.
pub(crate) fn raw_cap(size: u64) -> u64 {
    size.saturating_add(size / 255)
        .saturating_add(PAK_RAW_SLACK_BYTES)
}

fn read_table<R: Read + Seek>(part: &mut Part<R>, offset: u64) -> Result<Vec<Entry>, LspkError> {
    let head = read_range(&mut part.reader, part.len, offset, 8, "file table header")?;
    let num_files = le_u32(&head, 0);
    if num_files > MAX_FILES {
        return Err(LspkError::TooManyFiles(num_files));
    }
    let compressed_size = u64::from(le_u32(&head, 4));
    let expected = u64::from(num_files) * ENTRY_LEN as u64;
    if compressed_size > raw_cap(expected) {
        return Err(LspkError::FileTable(format!(
            "{compressed_size} compressed bytes is more than {num_files} entries can need"
        )));
    }
    let compressed = read_range(
        &mut part.reader,
        part.len,
        offset + 8,
        compressed_size,
        "file table",
    )?;
    decode_file_table(num_files, &compressed)
}

/// Read `len` bytes at `start`, after checking they lie inside a part of
/// `total` bytes — so a corrupt offset never allocates past the file.
fn read_range<R: Read + Seek>(
    r: &mut R,
    total: u64,
    start: u64,
    len: u64,
    what: &'static str,
) -> Result<Vec<u8>, LspkError> {
    let end = match start.checked_add(len) {
        Some(end) if end <= total => end,
        end => {
            return Err(LspkError::Truncated {
                what,
                start,
                end: end.unwrap_or(u64::MAX),
                len: total,
            });
        }
    };
    let io = |e: std::io::Error| LspkError::Io {
        what,
        reason: e.to_string(),
    };
    r.seek(SeekFrom::Start(start)).map_err(io)?;
    // `end - start <= total`, and `total` came from a seekable stream.
    let mut buf = vec![
        0u8;
        usize::try_from(end - start).map_err(|_| LspkError::Truncated {
            what,
            start,
            end,
            len: total
        })?
    ];
    r.read_exact(&mut buf).map_err(io)?;
    Ok(buf)
}

fn stream_len<R: Seek>(r: &mut R) -> Result<u64, LspkError> {
    r.seek(SeekFrom::End(0)).map_err(|e| LspkError::Io {
        what: "part length",
        reason: e.to_string(),
    })
}

fn part_count(count: usize) -> LspkError {
    LspkError::PartCount {
        count,
        limit: PAK_MAX_PARTS,
    }
}
