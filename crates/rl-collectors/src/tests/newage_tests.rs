//! `NewAge` (LSMF) reader tests on synthetic blobs (ROK-2078). No real save
//! bytes: [`lsmf`] writes the layout `newage.rs` documents.

use super::lsf_writer::{Attr, Codec, lsf};
use crate::{LSF_MAX_SECTION_BYTES, Lsmf, count_webp_images, read_lsf_attribute_bytes};

/// One synthetic component (index ≥ 1; `core.v0.Level` is added first).
#[derive(Clone)]
pub(crate) struct Comp {
    pub(crate) name: &'static str,
    pub(crate) version: u32,
    pub(crate) size: u32,
    pub(crate) data: Vec<u8>,
    pub(crate) owners: Option<Vec<u32>>,
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Header, name block, table, element data, entity GUIDs, indices, owner maps.
pub(crate) fn lsmf(comps: &[Comp], entities: u32) -> Vec<u8> {
    let level = Comp { name: "core.v0.Level", version: 0, size: 32, data: vec![0; 32], owners: None };
    let all: Vec<Comp> = std::iter::once(level).chain(comps.iter().cloned()).collect();
    let mut body: Vec<u8> = all.iter().flat_map(|c| c.name.bytes()).collect();
    let (names_len, table_at) = (body.len(), body.len());
    body.resize(table_at + all.len() * 48, 0);
    let mut offsets = Vec::new();
    for c in &all {
        offsets.push(body.len());
        body.extend_from_slice(&c.data);
    }
    let ent_start = body.len();
    for e in 0..entities {
        body.extend_from_slice(&[0xA0 | (e as u8 & 0x0F); 16]);
    }
    let ent_end = body.len();
    let mut maps = Vec::new();
    for (i, c) in all.iter().enumerate() {
        if let Some(o) = &c.owners {
            maps.push((body.len(), i, o.len()));
            o.iter().for_each(|x| body.extend_from_slice(&x.to_le_bytes()));
        }
    }
    let maps_start = body.len();
    for (s, i, n) in maps {
        for v in [s as u64, (s + n * 4) as u64, i as u64, n as u64] {
            body.extend_from_slice(&v.to_le_bytes());
        }
    }
    let maps_end = body.len();
    for (k, v) in [ent_start, ent_end, maps_start, maps_end].into_iter().enumerate() {
        put64(&mut body, offsets[0] + k * 8, v as u64);
    }
    let mut name_at = 0u64;
    for (i, c) in all.iter().enumerate() {
        let e = table_at + i * 48;
        put64(&mut body, e, name_at);
        put32(&mut body, e + 8, c.name.len() as u32);
        put32(&mut body, e + 12, 0xDEAD_BEEF);
        put32(&mut body, e + 24, c.size);
        put32(&mut body, e + 28, c.version);
        put64(&mut body, e + 32, (c.data.len() / c.size as usize) as u64);
        put64(&mut body, e + 40, offsets[i] as u64);
        name_at += c.name.len() as u64;
    }
    let mut out = vec![0u8; 48];
    out[..4].copy_from_slice(b"LSMF");
    put32(&mut out, 4, 1);
    put64(&mut out, 24, body.len() as u64);
    put32(&mut out, 32, names_len as u32);
    out[36..38].copy_from_slice(&(all.len() as u16).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Stats element: 8 unknown bytes, six i32 abilities, i32 proficiency.
pub(crate) fn stats(a: [i32; 6], prof: i32) -> Vec<u8> {
    let mut v = vec![0u8; 8];
    a.iter().chain([prof].iter()).for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
    v
}

fn sample() -> Vec<u8> {
    let mut stat_data = stats([8, 14, 12, 16, 10, 13], 2);
    stat_data.extend(stats([15, 12, 14, 8, 13, 10], 2));
    let comps = [
        Comp { name: "game.stats.v3.StatsComponent", version: 3, size: 36, data: stat_data, owners: Some(vec![2, 0]) },
        Comp { name: "game.v0.SideTable", version: 0, size: 4, data: vec![1; 8], owners: None },
    ];
    lsmf(&comps, 3)
}

#[test]
fn parses_the_component_table_and_owner_maps() {
    let blob = sample();
    let l = Lsmf::parse(&blob).expect("parse");
    let names: Vec<&str> = l.components().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["core.v0.Level", "game.stats.v3.StatsComponent", "game.v0.SideTable"]);
    assert_eq!(l.entity_count(), 3);
    let s = l.component("game.stats.v3.StatsComponent").expect("stats");
    assert_eq!((s.version, s.element_size, s.element_count), (3, 36, 2));
    let rows = l.by_entity("game.stats.v3.StatsComponent").expect("owned");
    assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [0, 2], "sorted by entity");
    assert_eq!(rows[0].1[8], 15, "entity 0 owns the second element");
    assert!(l.by_entity("game.v0.SideTable").is_none(), "a side table has no owners");
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    let blob = sample();
    for len in 0..blob.len() {
        assert!(Lsmf::parse(&blob[..len]).is_err(), "truncated to {len} bytes parsed");
    }
}

#[test]
fn garbage_and_corrupted_bytes_never_panic() {
    let blob = sample();
    for at in 0..blob.len() {
        for v in [0x00, 0xFF, 0x7F] {
            let mut b = blob.clone();
            b[at] = v;
            let _ = Lsmf::parse(&b).map(|l| l.by_entity("game.stats.v3.StatsComponent"));
        }
    }
    let mut seed = 0x2078_u64;
    for _ in 0..500 {
        let mut b: Vec<u8> = (0..256)
            .map(|_| {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                (seed >> 56) as u8
            })
            .collect();
        b[..4].copy_from_slice(b"LSMF");
        assert!(Lsmf::parse(&b).is_err());
    }
}

#[test]
fn an_owner_past_the_entity_table_is_refused() {
    let comps = [Comp { name: "game.x.v0.A", version: 0, size: 4, data: vec![0; 4], owners: Some(vec![9]) }];
    let err = Lsmf::parse(&lsmf(&comps, 2)).err().expect("refused");
    assert_eq!(err.0, "owner index is past the entity table");
}

#[test]
fn counts_embedded_webp_images() {
    let mut b = vec![0u8; 10];
    for _ in 0..3 {
        b.extend_from_slice(b"RIFF\x10\0\0\0WEBPVP8 ");
    }
    b.extend_from_slice(b"RIFF\x10\0\0\0WAVE");
    assert_eq!(count_webp_images(&b), 3);
}

#[test]
fn reads_a_blob_attribute_larger_than_the_section_cap() {
    let big = vec![0x5Au8; LSF_MAX_SECTION_BYTES as usize * 2];
    let attrs = [Attr { name: "Other", type_id: 4, value: vec![1, 2, 3, 4] }, Attr { name: "NewAge", type_id: 28, value: big.clone() }];
    for codec in [Codec::Stored, Codec::Zstd] {
        let file = lsf(&attrs, codec, false);
        let got = read_lsf_attribute_bytes(&file, "NewAge").expect("read").expect("present");
        assert_eq!(got.len(), big.len());
        assert!(got == big);
        assert!(read_lsf_attribute_bytes(&file, "Missing").expect("read").is_none());
    }
}
