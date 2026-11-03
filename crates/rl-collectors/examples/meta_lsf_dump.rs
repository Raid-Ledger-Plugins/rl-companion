//! Dev-only structural dump of a BG3 `meta.lsf` (ROK-2070 L2). Prints the
//! header, flags, section sizes and on-disk order, and every node and
//! attribute's name, type and length. Values are REDACTED, except the
//! GameID's type, length and an 8-hex prefix of its campaign key.
//!
//! `cargo run -p rl-collectors --example meta_lsf_dump -- <save.lsv|meta.lsf>`

#[path = "support/lsf_raw.rs"]
mod lsf_raw;

use lsf_raw::{NAMES, name, u32_at};

static LOADED: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: meta_lsf_dump <file>");
    let bytes = LOADED.get_or_init(|| lsf_raw::load(&path).expect("read meta.lsf"));
    let raw = lsf_raw::parse(bytes).expect("parse");
    let h = &raw.header;
    println!("meta.lsf: {} bytes", bytes.len());
    println!(
        "magic {:?} version {}",
        String::from_utf8_lossy(&h[..4]),
        u32_at(h, 4)
    );
    let engine = u64::from_le_bytes(h[8..16].try_into().unwrap());
    println!("engine u64 {engine:#018x}");
    for (i, (size, on_disk)) in raw.sizes.iter().enumerate() {
        println!(
            "  meta[{i}] {:<10} uncompressed {size:>6} onDisk {on_disk:>6} frame={}",
            NAMES[i], raw.is_frame[i]
        );
    }
    println!(
        "bytes 56..60 {:02x?}  flags {:#04x}  format u32 {}",
        &h[56..60],
        raw.flags,
        raw.format
    );
    let order: Vec<&str> = raw.order.iter().map(|&i| NAMES[i]).collect();
    println!("on-disk order: {order:?}  trailing bytes: {}", raw.trailing);
    let names = lsf_raw::strings(&raw.sections[0]);
    let total: usize = names.iter().map(Vec::len).sum();
    println!("strings: {} buckets, {total} names", names.len());
    let all: Vec<&str> = names.iter().flatten().map(String::as_str).collect();
    println!("  names: {all:?}");
    for &i in &raw.order {
        if raw.is_frame[i] {
            let b = |k| frame_byte(bytes, &raw, i, k);
            println!(
                "  {} LZ4 frame FLG {:#04x} BD {:#04x} HC {:#04x}",
                NAMES[i],
                b(4),
                b(5),
                b(6)
            );
        }
    }
    dump_nodes(&raw, &names);
    dump_attrs(&raw, &names);
    dump_keys(&raw, &names);
}

fn dump_nodes(raw: &lsf_raw::Raw, names: &[Vec<String>]) {
    let n = &raw.sections[2];
    let rec = if raw.format & 1 != 0 { 16 } else { 12 };
    println!(
        "nodes: {} bytes, {} records of {rec} B (rem {})",
        n.len(),
        n.len() / rec,
        n.len() % rec
    );
    for (i, r) in n.chunks_exact(rec).enumerate() {
        let f: Vec<i32> = (1..rec / 4).map(|k| u32_at(r, k * 4) as i32).collect();
        println!("  node[{i}] {} fields {f:?}", name(names, u32_at(r, 0)));
    }
}

fn dump_attrs(raw: &lsf_raw::Raw, names: &[Vec<String>]) {
    let (a, values) = (&raw.sections[3], &raw.sections[4]);
    let rec = if raw.format & 1 != 0 { 16 } else { 12 };
    println!(
        "attributes: {} bytes, {} records (rem {}); values {} bytes",
        a.len(),
        a.len() / rec,
        a.len() % rec,
        values.len()
    );
    let mut cumulative = 0usize;
    for (i, r) in a.chunks_exact(rec).enumerate() {
        let tl = u32_at(r, 4);
        let (ty, len) = (tl & 0x3F, (tl >> 6) as usize);
        let third = u32_at(r, 8) as i32;
        let offset = if rec == 16 {
            u32_at(r, 12) as usize
        } else {
            cumulative
        };
        cumulative += len;
        let nm = name(names, u32_at(r, 0));
        let v = values.get(offset..offset + len).unwrap_or(&[]);
        let shape = describe(v);
        println!(
            "  attr[{i}] {nm:<26} type {ty:>2} len {len:>4} third {third:>4} off {offset:>5} {shape}"
        );
        if ty == 31 && nm == "UUID" {
            uuid_check(v);
        }
        if nm == "GameID" {
            game_id(ty, v);
        }
    }
}

/// Redacted shape of a value: NUL position and printable-ASCII check only.
fn describe(v: &[u8]) -> String {
    let nul = v.iter().position(|&b| b == 0);
    let printable = v
        .iter()
        .take_while(|&&b| b != 0)
        .all(|b| (0x20..=0x7E).contains(b));
    format!("[first NUL {nul:?}, printable-before-NUL {printable}]")
}

fn game_id(ty: u32, v: &[u8]) {
    let lib = rl_collectors::read_lsf_attribute(LOADED.get().expect("loaded"), "GameID");
    println!(
        "  >> GameID type {ty} len {} library-read: {}",
        v.len(),
        match &lib {
            Ok(Some(t)) => format!(
                "Some(len {}) key prefix {}",
                t.len(),
                &rl_collectors::sha256_hex(t.as_bytes())[..8]
            ),
            Ok(None) => "None".into(),
            Err(e) => format!("Err({e})"),
        }
    );
}

fn dump_keys(raw: &lsf_raw::Raw, names: &[Vec<String>]) {
    let k = &raw.sections[1];
    println!("keys: {} bytes ({} u32 pairs?)", k.len(), k.len() / 8);
    for r in k.chunks_exact(8) {
        println!(
            "  key node {} attr-name {}",
            u32_at(r, 0),
            name(names, u32_at(r, 4))
        );
    }
}

/// Byte `k` of section `i` as stored on disk.
fn frame_byte(bytes: &[u8], raw: &lsf_raw::Raw, i: usize, k: usize) -> u8 {
    let mut at = 64;
    for &j in &raw.order {
        if j == i {
            return bytes[at + k];
        }
        let (size, on_disk) = raw.sizes[j];
        at += if on_disk == 0 { size } else { on_disk } as usize;
    }
    0
}

/// The base module's UUID is public game data (GustavDev); compare its bytes
/// with the LSLib display form under two candidate byte orders.
fn uuid_check(v: &[u8]) {
    // Known public base-module UUIDs: GustavDev (pre-patch 7), GustavX (7+).
    const KNOWN: [&str; 2] = [
        "28ac9ce2-2aba-8cda-b3b5-6e922f71b6b8",
        "cb555efe-2d9e-131f-8195-a89329d218ea",
    ];
    let lslib = [3, 2, 1, 0, 5, 4, 7, 6, 9, 8, 11, 10, 13, 12, 15, 14];
    let dotnet = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let raw: [usize; 16] = std::array::from_fn(|i| i);
    for (label, order) in [
        ("lslib-byteswap", lslib),
        ("plain-dotnet", dotnet),
        ("raw", raw),
    ] {
        let mut s = String::new();
        for (i, &at) in order.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                s.push('-');
            }
            s.push_str(&format!("{:02x}", v[at]));
        }
        println!(
            "  >> module UUID as {label}: matches a known base module = {}",
            KNOWN.contains(&s.as_str())
        );
    }
}
