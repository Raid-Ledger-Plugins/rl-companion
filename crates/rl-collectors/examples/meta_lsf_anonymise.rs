//! Dev-only: rebuild a BG3 `meta.lsf` with the real file's structure and
//! none of its data (ROK-2070 L2, spec D7). The transform is stated in
//! `tests/fixtures/meta/README.md`; keep the two in step.
//!
//! `cargo run -p rl-collectors --example meta_lsf_anonymise -- <save.lsv|meta.lsf> <out.lsf>`
//!
//! Before writing, every original string value, UUID and TranslatedString
//! handle (and, for an `.lsv`, every SaveInfo.json string) is searched for in
//! the output, compressed and decompressed. Any hit aborts; nothing is printed
//! but counts.

#[path = "support/lsf_raw.rs"]
mod lsf_raw;

use lsf_raw::{name, u32_at};

/// The synthetic GameID: same length as a real one (36 chars + NUL).
const SYNTHETIC_GAME_ID: &str = "00000000-2070-4000-8000-000000000001";
const STRING_TYPES: [u32; 6] = [20, 21, 22, 23, 29, 30];
const TRANSLATED_STRING: u32 = 28;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = match (args.get(1), args.get(2)) {
        (Some(i), Some(o)) => (i, o),
        _ => panic!("usage: meta_lsf_anonymise <save.lsv|meta.lsf> <out.lsf>"),
    };
    let bytes = lsf_raw::load(input).expect("read meta.lsf");
    let raw = lsf_raw::parse(&bytes).expect("parse");
    assert_eq!(
        raw.format & 1,
        0,
        "only V2 records (the real layout) are handled"
    );
    let names = lsf_raw::strings(&raw.sections[0]);
    let (values, mut needles) = anonymise_values(&raw, &names);
    needles.extend(save_info_strings(input));
    let out = rebuild(&raw, &values);
    let reparsed = lsf_raw::parse(&out).expect("output parses");
    let mut hay = out.clone();
    reparsed
        .sections
        .iter()
        .for_each(|s| hay.extend_from_slice(s));
    let hits = needles.iter().filter(|n| contains(&hay, n)).count();
    println!("checked {} needles; hits {hits}", needles.len());
    assert_eq!(hits, 0, "the output still carries an original value");
    std::fs::write(output, &out).expect("write output");
    println!("wrote {} bytes", out.len());
}

/// Rewrite the values section; return it with every original value worth
/// searching for (string text ≥ 3 bytes, UUIDs, handles).
fn anonymise_values(raw: &lsf_raw::Raw, names: &[Vec<String>]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut values = raw.sections[4].clone();
    let mut needles = Vec::new();
    let mut offset = 0usize;
    for rec in raw.sections[3].chunks_exact(12) {
        let tl = u32_at(rec, 4);
        let (ty, len) = (tl & 0x3F, (tl >> 6) as usize);
        let v = &mut values[offset..offset + len];
        offset += len;
        let text_end = v.iter().position(|&b| b == 0).unwrap_or(v.len());
        let attr = name(names, u32_at(rec, 0));
        if attr == "GameID" {
            assert!(STRING_TYPES.contains(&ty), "GameID is not a string type");
            assert_eq!(text_end, SYNTHETIC_GAME_ID.len(), "GameID length differs");
            needles.push(v[..text_end].to_vec());
            v[..text_end].copy_from_slice(SYNTHETIC_GAME_ID.as_bytes());
        } else if STRING_TYPES.contains(&ty) {
            push_needle(&mut needles, &v[..text_end]);
            v[..text_end].fill(b'x');
        } else if ty == TRANSLATED_STRING && len >= 6 && u32_at(v, 2) as usize + 6 == len {
            // u16 version, i32 handle length, handle text + NUL.
            push_needle(&mut needles, &v[6..len - 1]);
            v[6..len - 1].fill(b'x');
        } else {
            if len == 16 {
                needles.push(v.to_vec());
            }
            v.fill(0);
        }
    }
    assert_eq!(
        offset,
        values.len(),
        "attributes must cover the values section"
    );
    (values, needles)
}

fn push_needle(needles: &mut Vec<Vec<u8>>, text: &[u8]) {
    if text.len() >= 3 && !text.iter().all(|&b| b == b'x') {
        needles.push(text.to_vec());
    }
}

/// Every SaveInfo.json string of ≥ 3 bytes (character names and the like),
/// when the input is an `.lsv`; empty for a bare `.lsf`.
fn save_info_strings(path: &str) -> Vec<Vec<u8>> {
    let bytes = std::fs::read(path).expect("re-read input");
    let Ok(pkg) = rl_collectors::read_lspk(&bytes) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Ok(json) = rl_collectors::extract_save_info(&pkg) {
        collect(&json, &mut out);
    }
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str());
    out.extend(stem.map(|s| s.as_bytes().to_vec()));
    out
}

fn collect(v: &serde_json::Value, out: &mut Vec<Vec<u8>>) {
    match v {
        serde_json::Value::String(s) if s.len() >= 3 => out.push(s.as_bytes().to_vec()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect(x, out)),
        _ => {}
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Same header, flags and format; Strings as an LZ4 block, the rest as LZ4
/// frames like the original's; on-disk order Strings, Nodes, Attributes,
/// Values (Keys is empty in the real file and stays empty).
fn rebuild(raw: &lsf_raw::Raw, values: &[u8]) -> Vec<u8> {
    assert!(
        raw.sections[1].is_empty(),
        "a non-empty Keys section is not handled"
    );
    let mut out = raw.header.to_vec();
    let bodies = [
        (0, lz4_flex::block::compress(&raw.sections[0])),
        (2, lz4_frame(&raw.sections[2])),
        (3, lz4_frame(&raw.sections[3])),
        (4, lz4_frame(values)),
    ];
    for (slot, body) in &bodies {
        let slot = *slot;
        let size = if slot == 4 {
            values.len()
        } else {
            raw.sections[slot].len()
        };
        out[16 + slot * 8..20 + slot * 8].copy_from_slice(&(size as u32).to_le_bytes());
        out[20 + slot * 8..24 + slot * 8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    }
    for (_, body) in &bodies {
        out.extend_from_slice(body);
    }
    out
}

/// One LZ4 frame as BG3 writes them: FLG 0x40, BD 0x40, a real header
/// checksum, one block (stored if it does not shrink), end mark.
fn lz4_frame(data: &[u8]) -> Vec<u8> {
    let (flg, bd) = (0x40u8, 0x40u8);
    let mut out = vec![
        0x04,
        0x22,
        0x4D,
        0x18,
        flg,
        bd,
        (xxh32(&[flg, bd]) >> 8) as u8,
    ];
    let packed = lz4_flex::block::compress(data);
    if packed.len() < data.len() {
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&packed);
    } else {
        out.extend_from_slice(&(data.len() as u32 | 0x8000_0000).to_le_bytes());
        out.extend_from_slice(data);
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// xxHash32, seed 0, for inputs under 16 bytes (the frame descriptor).
fn xxh32(input: &[u8]) -> u32 {
    const P1: u32 = 2_654_435_761;
    const P2: u32 = 2_246_822_519;
    const P3: u32 = 3_266_489_917;
    const P4: u32 = 668_265_263;
    const P5: u32 = 374_761_393;
    assert!(input.len() < 16);
    let mut h = P5.wrapping_add(input.len() as u32);
    let mut chunks = input.chunks_exact(4);
    for c in &mut chunks {
        h = h
            .wrapping_add(u32_at(c, 0).wrapping_mul(P3))
            .rotate_left(17)
            .wrapping_mul(P4);
    }
    for &b in chunks.remainder() {
        h = h
            .wrapping_add(u32::from(b).wrapping_mul(P5))
            .rotate_left(11)
            .wrapping_mul(P1);
    }
    h ^= h >> 15;
    h = h.wrapping_mul(P2);
    h ^= h >> 13;
    h = h.wrapping_mul(P3);
    h ^ (h >> 16)
}
