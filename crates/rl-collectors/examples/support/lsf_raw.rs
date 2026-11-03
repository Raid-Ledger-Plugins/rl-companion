//! Dev-only LSF v7 structure parser shared by the `meta_lsf_*` examples
//! (ROK-2070 L2). Independent of `src/lsf.rs` on purpose, so the dump can
//! check the library's layout assumptions instead of repeating them.
#![allow(dead_code)]

use std::io::Read;

pub const NAMES: [&str; 5] = ["strings", "keys", "nodes", "attributes", "values"];
pub const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4D, 0x18];

/// One parsed resource: the 64-byte header, each section's metadata sizes,
/// its decompressed bytes, its on-disk order and its raw on-disk prefix.
pub struct Raw {
    pub header: [u8; 64],
    pub sizes: [(u32, u32); 5],
    pub flags: u8,
    pub format: u32,
    pub order: Vec<usize>,
    pub sections: [Vec<u8>; 5],
    pub is_frame: [bool; 5],
    pub trailing: usize,
}

/// Read `meta.lsf` bytes from either a raw `.lsf` or an `.lsv` package.
pub fn load(path: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.starts_with(b"LSOF") {
        return Ok(bytes);
    }
    let pkg = rl_collectors::read_lspk(&bytes).map_err(|e| e.to_string())?;
    pkg.read_file(rl_collectors::META_LSF_NAME)
        .map_err(|e| e.to_string())
}

pub fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn decode(raw: &[u8], size: usize, on_disk: u32, flags: u8) -> Result<Vec<u8>, String> {
    if on_disk == 0 || flags & 0x0F == 0 {
        return Ok(raw.to_vec());
    }
    match flags & 0x0F {
        2 if raw.starts_with(&LZ4_FRAME_MAGIC) => lz4_frame(raw, size),
        2 => lz4_flex::block::decompress(raw, size).map_err(|e| e.to_string()),
        3 => {
            let mut out = Vec::new();
            ruzstd::decoding::StreamingDecoder::new(raw)
                .map_err(|e| format!("{e:?}"))?
                .read_to_end(&mut out)
                .map_err(|e| e.to_string())?;
            Ok(out)
        }
        m => Err(format!("method {m}")),
    }
}

/// LZ4 frame decode (checksums skipped), mirroring the library's.
pub fn lz4_frame(raw: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let flg = *raw.get(4).ok_or("short")?;
    let block_ck = if flg & 0x10 != 0 { 4 } else { 0 };
    let mut at = 7 + if flg & 0x08 != 0 { 8 } else { 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let word = u32_at(raw.get(at..at + 4).ok_or("short")?, 0);
        at += 4;
        if word == 0 {
            return Ok(out);
        }
        let len = (word & 0x7FFF_FFFF) as usize;
        let block = raw.get(at..at + len).ok_or("short")?;
        at += len + block_ck;
        if word & 0x8000_0000 != 0 {
            out.extend_from_slice(block);
            continue;
        }
        let mut buf = vec![0u8; size.saturating_sub(out.len()).max(1)];
        let dict = out[out.len().saturating_sub(65_536)..].to_vec();
        let n = lz4_flex::block::decompress_into_with_dict(block, &mut buf, &dict)
            .map_err(|e| e.to_string())?;
        out.extend_from_slice(&buf[..n]);
    }
}

fn try_order(bytes: &[u8], raw: &mut Raw, order: &[usize]) -> Option<usize> {
    let mut at = 64;
    for &i in order {
        let (size, on_disk) = raw.sizes[i];
        let len = if on_disk == 0 || raw.flags & 0x0F == 0 {
            size
        } else {
            on_disk
        } as usize;
        let body = bytes.get(at..at + len)?;
        let out = decode(body, size as usize, on_disk, raw.flags).ok()?;
        if out.len() != size as usize {
            return None;
        }
        raw.is_frame[i] = body.starts_with(&LZ4_FRAME_MAGIC);
        raw.sections[i] = out;
        at += len;
    }
    Some(bytes.len() - at)
}

/// Parse, trying every on-disk order that starts with Strings; the first
/// order whose sections all decode to their declared sizes wins.
pub fn parse(bytes: &[u8]) -> Result<Raw, String> {
    let header: [u8; 64] = bytes.get(..64).ok_or("short header")?.try_into().unwrap();
    let mut raw = Raw {
        header,
        sizes: std::array::from_fn(|i| (u32_at(&header, 16 + i * 8), u32_at(&header, 20 + i * 8))),
        flags: header[56],
        format: u32_at(&header, 60),
        order: Vec::new(),
        sections: Default::default(),
        is_frame: [false; 5],
        trailing: 0,
    };
    for perm in permutations(&[1, 2, 3, 4]) {
        let order: Vec<usize> = std::iter::once(0).chain(perm).collect();
        if let Some(trailing) = try_order(bytes, &mut raw, &order) {
            raw.order = order;
            raw.trailing = trailing;
            return Ok(raw);
        }
    }
    Err("no section order decodes".into())
}

fn permutations(items: &[usize]) -> Vec<Vec<usize>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut all = Vec::new();
    for (i, &first) in items.iter().enumerate() {
        let rest: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, &v)| v)
            .collect();
        for mut p in permutations(&rest) {
            p.insert(0, first);
            all.push(p);
        }
    }
    all
}

/// String table as buckets of names.
pub fn strings(s: &[u8]) -> Vec<Vec<String>> {
    let buckets = u32_at(s, 0) as usize;
    let mut at = 4;
    let mut out = Vec::new();
    for _ in 0..buckets {
        let count = u16::from_le_bytes([s[at], s[at + 1]]) as usize;
        at += 2;
        let mut bucket = Vec::new();
        for _ in 0..count {
            let len = u16::from_le_bytes([s[at], s[at + 1]]) as usize;
            bucket.push(String::from_utf8_lossy(&s[at + 2..at + 2 + len]).into_owned());
            at += 2 + len;
        }
        out.push(bucket);
    }
    out
}

pub fn name(names: &[Vec<String>], r: u32) -> String {
    names
        .get((r >> 16) as usize)
        .and_then(|b| b.get((r & 0xFFFF) as usize))
        .cloned()
        .unwrap_or_else(|| format!("<bad ref {r:#x}>"))
}
