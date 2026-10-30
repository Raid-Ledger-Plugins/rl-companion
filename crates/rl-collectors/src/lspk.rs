//! LSPK v18 package reader (BG3 `.lsv` saves).
//!
//! Layout (ROK-1951 spike §2.1, matching LSLib `PackageFormat.cs`):
//!
//! | Offset | Field |
//! |---|---|
//! | 0 | magic `LSPK` |
//! | 4 | version u32 = 18 |
//! | 8 | FileListOffset u64 |
//! | 16 | FileListSize u32 |
//! | 20 | Flags u8, Priority u8 |
//! | 22 | MD5 \[16\] |
//! | 38 | NumParts u16 |
//!
//! At FileListOffset: `NumFiles u32`, `CompressedSize u32`, then an LZ4 block
//! that decompresses to `NumFiles × 272` bytes of `FileEntry18`:
//! `Name[256], OffsetInFile1 u32, OffsetInFile2 u16, ArchivePart u8, Flags u8,
//! SizeOnDisk u32, UncompressedSize u32`. The low nibble of `Flags` is the
//! compression method (0 none, 1 zlib, 2 LZ4, 3 zstd).

use std::io::Read;

use crate::error::LspkError;

/// `LSPK` magic.
pub const LSPK_MAGIC: [u8; 4] = *b"LSPK";
/// The only supported package version.
pub const LSPK_VERSION: u32 = 18;
/// Header length in bytes.
pub const HEADER_LEN: usize = 40;
/// One `FileEntry18` record in the decompressed file table.
pub const ENTRY_LEN: usize = 272;
const NAME_LEN: usize = 256;
/// The most file-table entries the reader accepts.
pub const MAX_FILES: u32 = 65_536;
/// Default cap on one decompressed entry (the largest save entry is ~48 MB).
pub const DEFAULT_MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
/// LZ4's largest possible expansion: one input byte stands for at most 255
/// output bytes, so fewer than `size / 255` compressed bytes cannot be real.
const LZ4_MAX_RATIO: u64 = 255;
/// A zstd frame may declare a window up to its entry's size, or this much
/// for smaller entries. 8 MiB (windowLog 23) is the largest window any
/// standard zstd level up to 19 picks, so a frame a real encoder made for a
/// small entry still fits; anything larger is refused before the decoder
/// reserves it (ruzstd itself allows windows up to 100 MiB).
const ZSTD_MIN_WINDOW_ALLOWANCE: u64 = 8 * 1024 * 1024;
/// Most bytes reserved up front for a zstd entry; the rest grows as decoded.
const ZSTD_PREALLOC_CAP: usize = 16 * 1024 * 1024;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Compression method, from the low nibble of an entry's flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMethod {
    None,
    Zlib,
    Lz4,
    Zstd,
    Unknown(u8),
}

impl CompressionMethod {
    /// Decode the method from an entry's flags byte.
    pub fn from_flags(flags: u8) -> Self {
        match flags & 0x0F {
            0 => Self::None,
            1 => Self::Zlib,
            2 => Self::Lz4,
            3 => Self::Zstd,
            other => Self::Unknown(other),
        }
    }
}

/// One file-table entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    pub archive_part: u8,
    pub flags: u8,
    pub size_on_disk: u32,
    pub uncompressed_size: u32,
}

impl Entry {
    /// The entry's compression method.
    pub fn compression(&self) -> CompressionMethod {
        CompressionMethod::from_flags(self.flags)
    }

    /// Size of the entry once decompressed.
    pub fn size(&self) -> u64 {
        match self.compression() {
            CompressionMethod::None => u64::from(self.size_on_disk),
            _ => u64::from(self.uncompressed_size),
        }
    }
}

/// A parsed package. Borrows the input; entries decompress on demand.
#[derive(Debug, Clone)]
pub struct Package<'a> {
    bytes: &'a [u8],
    /// Package version (always 18).
    pub version: u32,
    /// Header flags byte.
    pub flags: u8,
    /// Header priority byte.
    pub priority: u8,
    entries: Vec<Entry>,
}

/// Parse an LSPK v18 package: header and file table. Entry data is not
/// decompressed until [`Package::read_entry`] / [`Package::read_file`].
pub fn read_lspk(bytes: &[u8]) -> Result<Package<'_>, LspkError> {
    if let Some(m) = bytes.get(0..4)
        && m != LSPK_MAGIC
    {
        return Err(LspkError::BadMagic([m[0], m[1], m[2], m[3]]));
    }
    let h = slice(bytes, 0, HEADER_LEN as u64, "header")?;
    let version = le_u32(h, 4);
    if version != LSPK_VERSION {
        return Err(LspkError::UnsupportedVersion(version));
    }
    let num_parts = le_u16(h, 38);
    if num_parts > 1 {
        return Err(LspkError::UnsupportedMultiPart(num_parts));
    }
    let entries = read_file_table(bytes, le_u64(h, 8))?;
    Ok(Package {
        bytes,
        version,
        flags: h[20],
        priority: h[21],
        entries,
    })
}

impl Package<'_> {
    /// All file-table entries, in table order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Find an entry by exact name, ignoring ASCII case.
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// Decompress one entry, capped at [`DEFAULT_MAX_ENTRY_BYTES`].
    pub fn read_entry(&self, entry: &Entry) -> Result<Vec<u8>, LspkError> {
        self.read_entry_limited(entry, DEFAULT_MAX_ENTRY_BYTES)
    }

    /// Decompress one entry by name (ASCII case-insensitive).
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, LspkError> {
        let entry = self
            .find(name)
            .ok_or_else(|| LspkError::NotFound(name.to_owned()))?;
        self.read_entry(entry)
    }

    /// Decompress one entry, refusing anything larger than `limit` bytes
    /// before allocating for it.
    pub fn read_entry_limited(&self, entry: &Entry, limit: u64) -> Result<Vec<u8>, LspkError> {
        if entry.archive_part != 0 {
            return Err(LspkError::UnsupportedArchivePart {
                name: entry.name.clone(),
                part: entry.archive_part,
            });
        }
        let size = entry.size();
        let too_large = || LspkError::EntryTooLarge {
            name: entry.name.clone(),
            size,
            limit,
        };
        if size > limit {
            return Err(too_large());
        }
        let expected = usize::try_from(size).map_err(|_| too_large())?;
        let raw = slice(
            self.bytes,
            entry.offset,
            u64::from(entry.size_on_disk),
            "entry data",
        )?;
        let out = decode(entry, raw, expected)?;
        if out.len() != expected {
            return Err(LspkError::SizeMismatch {
                name: entry.name.clone(),
                expected: size,
                actual: out.len() as u64,
            });
        }
        Ok(out)
    }
}

fn decode(entry: &Entry, raw: &[u8], expected: usize) -> Result<Vec<u8>, LspkError> {
    let failed = |reason: String| LspkError::Decompress {
        name: entry.name.clone(),
        reason,
    };
    match entry.compression() {
        CompressionMethod::None => Ok(raw.to_vec()),
        CompressionMethod::Lz4 => {
            if !lz4_can_hold(raw.len() as u64, expected as u64) {
                return Err(failed(format!(
                    "{} compressed bytes cannot hold {expected} bytes",
                    raw.len()
                )));
            }
            lz4_flex::block::decompress(raw, expected).map_err(|e| failed(e.to_string()))
        }
        CompressionMethod::Zstd => zstd_decode(raw, expected).map_err(failed),
        CompressionMethod::Zlib | CompressionMethod::Unknown(_) => {
            Err(LspkError::UnsupportedCompression {
                name: entry.name.clone(),
                method: entry.flags & 0x0F,
            })
        }
    }
}

/// Whether `compressed` LZ4 bytes could possibly expand to `size` bytes.
fn lz4_can_hold(compressed: u64, size: u64) -> bool {
    compressed.saturating_mul(LZ4_MAX_RATIO) >= size
}

/// Decode one zstd frame. The frame header is checked first so a tiny entry
/// cannot make the decoder reserve a huge window; then at most
/// `expected + 1` bytes are read and the caller compares the length.
fn zstd_decode(raw: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let wanted = expected as u64;
    let window = zstd_window_size(raw)?;
    let allowance = wanted.max(ZSTD_MIN_WINDOW_ALLOWANCE);
    if window > allowance {
        return Err(format!(
            "zstd frame declares a {window}-byte window, over the {allowance}-byte allowance for this entry"
        ));
    }
    let decoder = ruzstd::decoding::StreamingDecoder::new(raw).map_err(|e| format!("{e:?}"))?;
    let mut out = Vec::with_capacity(expected.min(ZSTD_PREALLOC_CAP));
    decoder
        .take(wanted + 1)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// The window size a zstd frame header declares (RFC 8878 §3.1.1.1); for a
/// single-segment frame that is its content size. A content size that
/// disagrees with the file table surfaces later as a size mismatch.
fn zstd_window_size(raw: &[u8]) -> Result<u64, String> {
    if raw.get(..4) != Some(&ZSTD_MAGIC[..]) {
        return Err("not a zstd frame (bad magic)".to_owned());
    }
    let short = || "zstd frame header is truncated".to_owned();
    let descriptor = *raw.get(4).ok_or_else(short)?;
    let single_segment = descriptor & 0x20 != 0;
    let mut at = 5;
    let mut window = None;
    if !single_segment {
        let wd = *raw.get(at).ok_or_else(short)?;
        at += 1;
        let base = 1u64 << (10 + u32::from(wd >> 3));
        window = Some(base + (base / 8) * u64::from(wd & 7));
    }
    at += match descriptor & 0x03 {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    let fcs_len = match descriptor >> 6 {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut content = None;
    if fcs_len > 0 {
        let field = raw.get(at..at + fcs_len).ok_or_else(short)?;
        let mut le = [0u8; 8];
        le[..fcs_len].copy_from_slice(field);
        let n = u64::from_le_bytes(le);
        content = Some(if fcs_len == 2 { n + 256 } else { n });
    }
    // A single-segment frame always carries its content size.
    window.or(content).ok_or_else(short)
}

fn read_file_table(bytes: &[u8], offset: u64) -> Result<Vec<Entry>, LspkError> {
    let head = slice(bytes, offset, 8, "file table header")?;
    let num_files = le_u32(head, 0);
    if num_files > MAX_FILES {
        return Err(LspkError::TooManyFiles(num_files));
    }
    let compressed_size = u64::from(le_u32(head, 4));
    let compressed = slice(bytes, offset + 8, compressed_size, "file table")?;
    let expected = num_files as usize * ENTRY_LEN;
    if !lz4_can_hold(compressed_size, expected as u64) {
        return Err(LspkError::FileTable(format!(
            "{compressed_size} compressed bytes cannot hold {num_files} entries ({expected} bytes)"
        )));
    }
    let table = lz4_flex::block::decompress(compressed, expected)
        .map_err(|e| LspkError::FileTable(e.to_string()))?;
    if table.len() != expected {
        return Err(LspkError::FileTable(format!(
            "decompressed to {} bytes, expected {expected}",
            table.len()
        )));
    }
    Ok(table.chunks_exact(ENTRY_LEN).map(parse_entry).collect())
}

fn parse_entry(raw: &[u8]) -> Entry {
    let name_bytes = &raw[..NAME_LEN];
    let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    let offset = u64::from(le_u32(raw, 256)) | (u64::from(le_u16(raw, 260)) << 32);
    Entry {
        name: String::from_utf8_lossy(&name_bytes[..end]).into_owned(),
        offset,
        archive_part: raw[262],
        flags: raw[263],
        size_on_disk: le_u32(raw, 264),
        uncompressed_size: le_u32(raw, 268),
    }
}

/// Bounds-checked `bytes[start..start + len]`.
fn slice<'a>(
    bytes: &'a [u8],
    start: u64,
    len: u64,
    what: &'static str,
) -> Result<&'a [u8], LspkError> {
    let total = bytes.len() as u64;
    match start.checked_add(len) {
        // start <= end <= total, and total fits in usize.
        Some(end) if end <= total => Ok(&bytes[start as usize..end as usize]),
        end => Err(LspkError::Truncated {
            what,
            start,
            end: end.unwrap_or(u64::MAX),
            len: total,
        }),
    }
}

// The helpers below are only called on slices whose length was checked.
fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le_u64(b: &[u8], at: usize) -> u64 {
    u64::from(le_u32(b, at)) | (u64::from(le_u32(b, at + 4)) << 32)
}
