//! Test-only LSF v7 writer: the layout `lsf.rs` reads (ROK-2070 spec §2), so
//! resources round-trip. Synthetic values only — no real save bytes.

use super::writer::{set_u32, zstd_raw_frame};

/// LSF type ids used by the tests.
pub(crate) const FIXED_STRING: u32 = 22;
pub(crate) const UUID: u32 = 31;
pub(crate) const INT32: u32 = 4;

/// A synthetic string GameID and a UUID one.
pub(crate) const STRING_GAME_ID: &str = "synthetic-game-id-0001";
pub(crate) const UUID_BYTES: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
/// [`UUID_BYTES`] in LSLib's display form.
pub(crate) const UUID_TEXT: &str = "03020100-0504-0706-0908-0b0a0d0c0f0e";

/// One attribute on the single `MetaData` node.
#[derive(Clone)]
pub(crate) struct Attr {
    pub(crate) name: &'static str,
    pub(crate) type_id: u32,
    pub(crate) value: Vec<u8>,
}

/// A NUL-terminated FixedString attribute.
pub(crate) fn string_attr(name: &'static str, text: &str) -> Attr {
    let mut value = text.as_bytes().to_vec();
    value.push(0);
    Attr {
        name,
        type_id: FIXED_STRING,
        value,
    }
}

pub(crate) fn uuid_attr(name: &'static str, bytes: [u8; 16]) -> Attr {
    Attr {
        name,
        type_id: UUID,
        value: bytes.to_vec(),
    }
}

/// A `meta.lsf`-like attribute list with the GameID in the middle.
pub(crate) fn meta_attrs(game_id: Attr) -> Vec<Attr> {
    vec![
        string_attr("LevelUniqueKey", "WLD_Main_A_synthetic"),
        game_id,
        string_attr("SaveTime", "2026-11-02T00:00:00"),
        Attr {
            name: "HonorDifficultySelected",
            type_id: INT32,
            value: 0u32.to_le_bytes().to_vec(),
        },
    ]
}

/// Uncompressed section bodies, editable before [`write_lsf`].
#[derive(Clone)]
pub(crate) struct Sections {
    pub(crate) strings: Vec<u8>,
    pub(crate) nodes: Vec<u8>,
    pub(crate) attrs: Vec<u8>,
    pub(crate) values: Vec<u8>,
    pub(crate) keys: Vec<u8>,
}

/// How sections are stored on disk.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Codec {
    /// `onDisk = 0`, flags 0.
    Stored,
    /// Flags 0x22: strings as an LZ4 block, the rest as LZ4 frames with the
    /// given FLG byte, split into `chunk`-byte blocks.
    Lz4 { flg: u8, chunk: usize },
    /// Flags 0x23: zstd frames of raw blocks.
    Zstd,
    /// The given flags; bytes written as-is with `onDisk = size`.
    Raw(u8),
}

/// The LZ4 default: independent blocks, no checksums, no content size.
pub(crate) const LZ4: Codec = Codec::Lz4 {
    flg: 0x60,
    chunk: 4096,
};

/// String table with three buckets; name `i` lives in bucket `i % 3`.
fn names_of(attrs: &[Attr]) -> (Vec<u8>, Vec<&'static str>) {
    let mut names = vec!["MetaData"];
    for a in attrs {
        if !names.contains(&a.name) {
            names.push(a.name);
        }
    }
    let mut out = 3u32.to_le_bytes().to_vec();
    for bucket in 0..3 {
        let members: Vec<&str> = names.iter().skip(bucket).step_by(3).copied().collect();
        out.extend_from_slice(&(members.len() as u16).to_le_bytes());
        for name in members {
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
        }
    }
    (out, names)
}

/// The name ref `(bucket << 16) | index` of `names[i]`.
fn name_ref(names: &[&str], name: &str) -> u32 {
    let i = names.iter().position(|n| *n == name).expect("name") as u32;
    ((i % 3) << 16) | (i / 3)
}

/// Build the sections. V3 stores values in reverse so its explicit offsets
/// differ from the cumulative ones V2 implies.
pub(crate) fn build_sections(attrs: &[Attr], v3: bool) -> Sections {
    let (strings, names) = names_of(attrs);
    let mut nodes = name_ref(&names, "MetaData").to_le_bytes().to_vec();
    let node_tail: [i32; 3] = if v3 { [-1, -1, 0] } else { [0, -1, i32::MIN] };
    let tail_len = if v3 { 3 } else { 2 };
    for v in &node_tail[..tail_len] {
        nodes.extend_from_slice(&v.to_le_bytes());
    }
    let order: Vec<usize> = if v3 {
        (0..attrs.len()).rev().collect()
    } else {
        (0..attrs.len()).collect()
    };
    let mut values = Vec::new();
    let mut offsets = vec![0u32; attrs.len()];
    for &i in &order {
        offsets[i] = values.len() as u32;
        values.extend_from_slice(&attrs[i].value);
    }
    let mut records = Vec::new();
    for (i, a) in attrs.iter().enumerate() {
        records.extend_from_slice(&name_ref(&names, a.name).to_le_bytes());
        let type_and_len = a.type_id | ((a.value.len() as u32) << 6);
        records.extend_from_slice(&type_and_len.to_le_bytes());
        if v3 {
            let next = if i + 1 < attrs.len() {
                i as i32 + 1
            } else {
                -1
            };
            records.extend_from_slice(&next.to_le_bytes());
            records.extend_from_slice(&offsets[i].to_le_bytes());
        } else {
            records.extend_from_slice(&0i32.to_le_bytes());
        }
    }
    Sections {
        strings,
        nodes,
        attrs: records,
        values,
        keys: Vec::new(),
    }
}

/// One LZ4 frame: magic, FLG, BD 0x40, optional content size, a dummy header
/// checksum, then blocks (linked when FLG bit 5 is clear), end mark and an
/// optional dummy content checksum.
pub(crate) fn lz4_frame(data: &[u8], flg: u8, chunk: usize) -> Vec<u8> {
    let mut out = vec![0x04, 0x22, 0x4D, 0x18, flg, 0x40];
    if flg & 0x08 != 0 {
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    }
    out.push(0xA5);
    for (i, block) in data.chunks(chunk).enumerate() {
        let packed = if flg & 0x20 == 0 {
            lz4_flex::block::compress_with_dict(block, &data[..i * chunk])
        } else {
            lz4_flex::block::compress(block)
        };
        if packed.len() < block.len() {
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&packed);
        } else {
            out.extend_from_slice(&(block.len() as u32 | 0x8000_0000).to_le_bytes());
            out.extend_from_slice(block);
        }
        if flg & 0x10 != 0 {
            out.extend_from_slice(&[0xC5; 4]);
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    if flg & 0x04 != 0 {
        out.extend_from_slice(&[0xC5; 4]);
    }
    out
}

/// Encode one section: `(on-disk bytes, onDisk field)`.
fn encode(data: &[u8], codec: Codec, is_strings: bool) -> (Vec<u8>, u32) {
    if data.is_empty() {
        return (Vec::new(), 0);
    }
    let packed = match codec {
        Codec::Stored => return (data.to_vec(), 0),
        Codec::Lz4 { .. } if is_strings => lz4_flex::block::compress(data),
        Codec::Lz4 { flg, chunk } => lz4_frame(data, flg, chunk),
        Codec::Zstd => zstd_raw_frame(data),
        Codec::Raw(_) => data.to_vec(),
    };
    let on_disk = packed.len() as u32;
    (packed, on_disk)
}

fn flags_of(codec: Codec) -> u8 {
    match codec {
        Codec::Stored => 0,
        Codec::Lz4 { .. } => 0x22,
        Codec::Zstd => 0x23,
        Codec::Raw(flags) => flags,
    }
}

/// Header (version 7), metadata, then strings, nodes, attributes, values, keys.
pub(crate) fn write_lsf(s: &Sections, codec: Codec, v3: bool) -> Vec<u8> {
    let mut out = vec![0u8; 64];
    out[..4].copy_from_slice(b"LSOF");
    set_u32(&mut out, 4, 7);
    out[8..16].copy_from_slice(&0x0004_0008_0000_02BC_u64.to_le_bytes());
    let parts = [
        (&s.strings, 0usize, true),
        (&s.keys, 1, false),
        (&s.nodes, 2, false),
        (&s.attrs, 3, false),
        (&s.values, 4, false),
    ];
    let mut bodies = vec![Vec::new(); 5];
    for (data, slot, is_strings) in parts {
        let (packed, on_disk) = encode(data, codec, is_strings);
        set_u32(&mut out, 16 + slot * 8, data.len() as u32);
        set_u32(&mut out, 20 + slot * 8, on_disk);
        bodies[slot] = packed;
    }
    out[56] = flags_of(codec);
    set_u32(&mut out, 60, u32::from(v3));
    for slot in [0, 2, 3, 4, 1] {
        out.extend_from_slice(&bodies[slot]);
    }
    out
}

/// Shorthand: attributes → sections → resource.
pub(crate) fn lsf(attrs: &[Attr], codec: Codec, v3: bool) -> Vec<u8> {
    write_lsf(&build_sections(attrs, v3), codec, v3)
}
