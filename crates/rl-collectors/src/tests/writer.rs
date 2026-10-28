//! Test-only LSPK v18 writer: the same layout the reader parses (header,
//! entry data, LZ4 file table last), so packages round-trip.

use crate::lspk::{ENTRY_LEN, HEADER_LEN, LSPK_VERSION};

/// Byte offsets inside one 272-byte `FileEntry18` record.
pub(crate) const ENTRY_OFFSET1: usize = 256;
pub(crate) const ENTRY_PART: usize = 262;
pub(crate) const ENTRY_SIZE_ON_DISK: usize = 264;
pub(crate) const ENTRY_UNCOMPRESSED: usize = 268;

/// How an entry is stored.
#[derive(Clone, Copy)]
pub(crate) enum Method {
    /// Flags 0: bytes as-is.
    Stored,
    /// Flags 0x22: one LZ4 block.
    Lz4,
    /// Flags 0x23: a zstd frame made of raw blocks.
    ZstdRaw,
    /// Flags 0x21: zlib (the reader refuses it; data written as-is).
    Zlib,
    /// Given flags and on-disk bytes; `data` only supplies the length.
    Precompressed(u8, &'static [u8]),
}

pub(crate) struct FileSpec<'a> {
    pub(crate) name: &'a str,
    pub(crate) data: &'a [u8],
    pub(crate) method: Method,
}

pub(crate) fn write_lspk(files: &[FileSpec<'_>]) -> Vec<u8> {
    write_lspk_with(files, |_| {})
}

/// Like [`write_lspk`]; `tamper` edits the uncompressed file table first.
pub(crate) fn write_lspk_with(files: &[FileSpec<'_>], tamper: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_LEN];
    let mut table = Vec::with_capacity(files.len() * ENTRY_LEN);
    for file in files {
        let offset = out.len() as u64;
        let (stored, flags) = match file.method {
            Method::Stored => (file.data.to_vec(), 0u8),
            Method::Lz4 => (lz4_flex::block::compress(file.data), 0x22),
            Method::ZstdRaw => (zstd_raw_frame(file.data), 0x23),
            Method::Zlib => (file.data.to_vec(), 0x21),
            Method::Precompressed(flags, bytes) => (bytes.to_vec(), flags),
        };
        out.extend_from_slice(&stored);
        let mut name = [0u8; 256];
        name[..file.name.len()].copy_from_slice(file.name.as_bytes());
        table.extend_from_slice(&name);
        table.extend_from_slice(&(offset as u32).to_le_bytes());
        table.extend_from_slice(&((offset >> 32) as u16).to_le_bytes());
        table.push(0);
        table.push(flags);
        table.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        table.extend_from_slice(&(file.data.len() as u32).to_le_bytes());
    }
    tamper(&mut table);
    let list_offset = out.len() as u64;
    let compressed = lz4_flex::block::compress(&table);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    out[0..4].copy_from_slice(b"LSPK");
    set_u32(&mut out, 4, LSPK_VERSION);
    set_u64(&mut out, 8, list_offset);
    set_u32(&mut out, 16, 8 + compressed.len() as u32);
    set_u16(&mut out, 38, 1);
    out
}

/// A single-segment zstd frame of raw (uncompressed) blocks, no checksum.
pub(crate) fn zstd_raw_frame(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 128 * 1024;
    // Magic, then descriptor 0xA0: 4-byte content size, single segment.
    let mut out = vec![0x28, 0xB5, 0x2F, 0xFD, 0xA0];
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    let chunks: Vec<&[u8]> = data.chunks(MAX_BLOCK).collect();
    let last = chunks.len().saturating_sub(1);
    for (i, chunk) in chunks.iter().enumerate() {
        // Block header: bit 0 last, bits 1-2 type (0 = raw), bits 3+ size.
        let header = ((chunk.len() as u32) << 3) | u32::from(i == last);
        out.extend_from_slice(&header.to_le_bytes()[..3]);
        out.extend_from_slice(chunk);
    }
    out
}

pub(crate) fn set_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn set_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn set_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn get_u64(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}
