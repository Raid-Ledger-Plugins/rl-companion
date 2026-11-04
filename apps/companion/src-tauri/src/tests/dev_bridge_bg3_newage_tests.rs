//! ROK-2078 layer 2: the LSMF decoder behind `/dev/v1/bg3/saves/<ref>/structure`
//! on a synthetic save (a stored LSF holding a synthetic `NewAge`; no real
//! save bytes). The LSMF layout is the one `rl_collectors::Lsmf` documents.

use super::bg3_collector_tests::lspk;
use super::dev_bridge_unit_tests::schema_errors;
use super::temp_dir;
use crate::dev_bridge::bg3_newage_reader::LsmfDecoder;
use crate::dev_bridge::bg3_redact::body_is_clean;
use crate::dev_bridge::bg3_routes;
use crate::dev_bridge::bg3_saves::{save_ref, Bg3Source};
use crate::dev_bridge::paging::parse_query;
use serde_json::{json, Value};
use std::sync::Arc;

const SAVE_INFO: &[u8] =
    include_bytes!("../../../../../contracts/bg3-save/v1/fixtures/saveinfo/act2-party-4p-summon.saveinfo.json");
const META_LSF: &[u8] =
    include_bytes!("../../../../../crates/rl-collectors/tests/fixtures/meta/real-shape-string.meta.lsf");
const SOLO_INFO: &[u8] =
    include_bytes!("../../../../../contracts/bg3-save/v1/fixtures/saveinfo/act1-solo-no-thumbnail.saveinfo.json");
const KEY: [u8; 32] = [9; 32];
const PARTY: u32 = 5;

/// A stored (uncompressed) LSF v7 with one `NewAge` ScratchBuffer attribute.
fn globals_lsf(newage: &[u8]) -> Vec<u8> {
    let mut strings = 1u32.to_le_bytes().to_vec();
    strings.extend_from_slice(&1u16.to_le_bytes());
    strings.extend_from_slice(&6u16.to_le_bytes());
    strings.extend_from_slice(b"NewAge");
    let mut attr = 0u32.to_le_bytes().to_vec();
    attr.extend_from_slice(&(((newage.len() as u32) << 6) | 28).to_le_bytes());
    attr.extend_from_slice(&0i32.to_le_bytes());
    let mut out = vec![0u8; 64];
    out[..4].copy_from_slice(b"LSOF");
    out[4..8].copy_from_slice(&7u32.to_le_bytes());
    for (slot, len) in [(0usize, strings.len()), (3, attr.len()), (4, newage.len())] {
        out[16 + slot * 8..20 + slot * 8].copy_from_slice(&(len as u32).to_le_bytes());
    }
    [out, strings, attr, newage.to_vec()].concat()
}

fn put(b: &mut [u8], at: usize, v: u64, n: usize) {
    b[at..at + n].copy_from_slice(&v.to_le_bytes()[..n]);
}

/// `(name, elementSize, element bytes, owners)`; `core.v0.Level` is added first.
fn lsmf(comps: &[(&str, u32, Vec<u8>, Vec<u32>)], entities: usize) -> Vec<u8> {
    let mut all = vec![("core.v0.Level", 32u32, vec![0u8; 32], None)];
    all.extend(comps.iter().map(|(n, s, d, o)| (*n, *s, d.clone(), Some(o.clone()))));
    let mut body: Vec<u8> = all.iter().flat_map(|c| c.0.bytes()).collect();
    let table = body.len();
    body.resize(table + all.len() * 48, 0);
    let offsets: Vec<usize> = all.iter().map(|c| { let at = body.len(); body.extend(&c.2); at }).collect();
    let ents = body.len();
    body.resize(ents + entities * 16, 0x11);
    let mut maps = Vec::new();
    for (i, c) in all.iter().enumerate() {
        if let Some(o) = &c.3 {
            maps.push((body.len(), i, o.len()));
            o.iter().for_each(|x| body.extend(x.to_le_bytes()));
        }
    }
    let maps_at = body.len();
    for (s, i, n) in maps {
        [s, s + n * 4, i, n].iter().for_each(|v| body.extend((*v as u64).to_le_bytes()));
    }
    for (k, v) in [ents, ents + entities * 16, maps_at, body.len()].into_iter().enumerate() {
        put(&mut body, offsets[0] + k * 8, v as u64, 8);
    }
    let mut name_at = 0;
    for (i, c) in all.iter().enumerate() {
        let e = table + i * 48;
        put(&mut body, e, name_at as u64, 8);
        put(&mut body, e + 8, c.0.len() as u64, 4);
        put(&mut body, e + 24, u64::from(c.1), 4);
        put(&mut body, e + 32, (c.2.len() / c.1 as usize) as u64, 8);
        put(&mut body, e + 40, offsets[i] as u64, 8);
        name_at += c.0.len();
    }
    let mut out = vec![0u8; 48];
    out[..4].copy_from_slice(b"LSMF");
    put(&mut out, 24, body.len() as u64, 8);
    put(&mut out, 32, table as u64, 4);
    put(&mut out, 36, all.len() as u64, 2);
    [out, body].concat()
}

/// Five party entities (2..=6) with Stats and Health; `stats_size` drifts the layout.
fn party_blob(stats_size: u32) -> Vec<u8> {
    party_blob_of(stats_size, PARTY)
}

fn party_blob_of(stats_size: u32, party: u32) -> Vec<u8> {
    let owners: Vec<u32> = (2..2 + party).collect();
    let mut stats = Vec::new();
    let mut health = Vec::new();
    for (i, _) in owners.iter().enumerate() {
        let mut s = vec![0u8; 8];
        for a in [10 + i as i32, 14, 12, 8, 13, 15, 3] {
            s.extend(a.to_le_bytes());
        }
        s.resize(stats_size as usize, 0);
        stats.extend(s);
        let mut h = [20i32, 40 + i as i32].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        h.resize(32, 0);
        health.extend(h);
    }
    let comps = [
        ("game.v0.PlayerComponent", 1, vec![1u8; party as usize], owners.clone()),
        ("game.stats.v3.StatsComponent", stats_size, stats, owners.clone()),
        ("game.stats.v0.HealthComponent", 32, health, owners),
    ];
    let mut blob = lsmf(&comps, 8);
    blob.extend_from_slice(b"RIFF\x10\0\0\0WEBPVP8 ");
    blob
}

fn source(tag: &str, newage: &[u8]) -> (Bg3Source, String) {
    source_with_globals(tag, globals_lsf(newage))
}

fn source_with_globals(tag: &str, globals: Vec<u8>) -> (Bg3Source, String) {
    source_full(tag, SAVE_INFO, globals)
}

fn source_full(tag: &str, save_info: &[u8], globals: Vec<u8>) -> (Bg3Source, String) {
    source_files(tag, save_info, globals, META_LSF)
}

fn source_files(tag: &str, save_info: &[u8], globals: Vec<u8>, meta: &[u8]) -> (Bg3Source, String) {
    let story = temp_dir(tag);
    let dir = story.join("QuickSave_7");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let files: Vec<(&str, &[u8])> = vec![("SaveInfo.json", save_info), ("Globals.lsf", &globals), ("meta.lsf", meta)];
    std::fs::write(dir.join("QuickSave_7.lsv"), lspk(&files)).expect("write lsv");
    let r = save_ref(&KEY, &story, "QuickSave_7/QuickSave_7.lsv");
    (Bg3Source::with_key(vec![story], KEY, Arc::new(LsmfDecoder)), r)
}

fn get(src: &Bg3Source, path: &str) -> Value {
    let (status, body) = bg3_routes::get(Some(src), path, &parse_query(Some(""))).expect("bg3 route");
    assert_eq!(status, 200, "{path} → {body}");
    body
}

fn structure(src: &Bg3Source, r: &str) -> Value {
    let body = get(src, &format!("/dev/v1/bg3/saves/{r}/structure"));
    assert_eq!(schema_errors("bg3-save-structure", &body), Vec::<String>::new());
    assert!(body_is_clean(&body), "{body}");
    assert!(body["next"].is_null(), "fits one page");
    body
}

#[test]
fn a_synthetic_save_fills_types_checks_and_party_stats() {
    let (src, r) = source("bg3-newage-ok", &party_blob(36));
    let body = structure(&src, &r);
    let types = body["types"].as_array().expect("types");
    assert_eq!(types.len(), 4, "{types:?}");
    assert_eq!(types[2], json!({ "type": "game.stats.v3.StatsComponent", "version": 0,
        "elementSize": 36, "count": 5, "decoder": "ok" }));
    assert_eq!(types[0]["decoder"], json!("none"), "core.v0.Level is not decoded");
    assert_eq!(body["checks"]["partyEntitiesFound"], json!(5));
    // Codex P2: five members in entity order are NOT proven to be SaveInfo
    // order, so the match is not claimed and (review MINOR) no slot carries
    // values that might belong to another character.
    assert_eq!(body["checks"]["partyMatchesSaveInfo"], json!(false), "order unproven for 5");
    assert_eq!(body["checks"]["portraitsFound"], json!(1));
    for k in 0..5 {
        assert_eq!(body["party"][k], json!({ "slot": k, "decoded": {} }), "slot {k} unproven");
    }
    let saves = get(&src, "/dev/v1/bg3/saves");
    assert_eq!(saves["saves"][0]["fallbackReason"], Value::Null);
}

#[test]
fn layout_drift_marks_the_type_unsupported_and_decodes_nothing_from_it() {
    let (src, r) = source_full("bg3-newage-drift", SOLO_INFO, globals_lsf(&party_blob_of(40, 1)));
    let body = structure(&src, &r);
    assert_eq!(body["types"][2]["decoder"], json!("unsupported"), "{}", body["types"]);
    assert_eq!(body["party"][0]["decoded"], json!({ "maxHp": 40 }), "stats skipped, health kept");
}

#[test]
fn a_garbage_or_truncated_newage_is_unreadable_not_a_crash() {
    let full = party_blob(36);
    for (i, blob) in [b"LSMF garbage".to_vec(), full[..full.len() / 2].to_vec(), vec![0xFF; 300]].iter().enumerate() {
        let (src, r) = source(&format!("bg3-newage-bad-{i}"), blob);
        let body = structure(&src, &r);
        assert_eq!(body["types"], json!([]));
        assert_eq!(body["party"][0], json!({ "slot": 0, "decoded": {} }));
        let saves = get(&src, "/dev/v1/bg3/saves");
        assert_eq!(saves["saves"][0]["fallbackReason"], json!("newage-unreadable"));
    }
}

/// Codex P2 / review MINOR: the attribute table reads but the values section
/// is truncated. The listing must say `lsf-unreadable`, not advertise a full
/// read (`null`) while the structure read comes back empty.
#[test]
fn a_truncated_values_section_is_lsf_unreadable_in_the_listing() {
    let mut globals = globals_lsf(&party_blob(36));
    globals.truncate(globals.len() - 64);
    let (src, r) = source_with_globals("bg3-newage-lsf-cut", globals);
    let body = structure(&src, &r);
    assert_eq!(body["types"], json!([]), "the structure read decodes nothing");
    let saves = get(&src, "/dev/v1/bg3/saves");
    assert_eq!(saves["saves"][0]["fallbackReason"], json!("lsf-unreadable"), "{saves}");
    assert_eq!(saves["saves"][0]["lsfVersions"]["globals"], json!(null), "an undecodable Globals reports no version: {saves}");
}

/// Codex P2: a one-member party is the only case whose slot correspondence
/// is proven, so it is the only one that claims `partyMatchesSaveInfo`.
#[test]
fn a_solo_party_claims_the_save_info_match_and_decodes_slot_0() {
    let (src, r) = source_full("bg3-newage-solo", SOLO_INFO, globals_lsf(&party_blob_of(36, 1)));
    let body = structure(&src, &r);
    assert_eq!(body["checks"]["partyEntitiesFound"], json!(1));
    assert_eq!(body["checks"]["partyMatchesSaveInfo"], json!(true), "one member: slot 0 is proven");
    assert_eq!(body["party"][0]["decoded"], json!({
        "abilities": { "str": 10, "dex": 14, "con": 12, "int": 8, "wis": 13, "cha": 15 },
        "proficiencyBonus": 3, "maxHp": 40 }));
}

/// Review MINOR: the LSF reads fine but `NewAge` is over the 32 MiB cap
/// (a forged attribute length) — the blob, not the LSF, is unreadable.
#[test]
fn an_over_cap_newage_is_newage_unreadable() {
    let mut globals = globals_lsf(&party_blob(36));
    let len_at = 64 + 14 + 4; // header, the 14-byte strings section, the name ref
    let forged = ((rl_collectors::LSMF_MAX_BYTES as u32 + 1) << 6) | 28;
    globals[len_at..len_at + 4].copy_from_slice(&forged.to_le_bytes());
    let (src, r) = source_with_globals("bg3-newage-over-cap", globals);
    assert_eq!(structure(&src, &r)["types"], json!([]), "nothing decoded");
    let row = &get(&src, "/dev/v1/bg3/saves")["saves"][0];
    assert_eq!(row["fallbackReason"], json!("newage-unreadable"), "{row}");
    assert_eq!(row["lsfVersions"]["globals"], json!(7), "the LSF itself parsed");
}

/// Codex P2: a meta.lsf that keeps its `LSOF` + version prefix but is
/// truncated past it is unreadable — no version, `lsf-unreadable`.
#[test]
fn a_truncated_meta_lsf_is_lsf_unreadable_with_no_version() {
    let globals = globals_lsf(&party_blob(36));
    let (src, _) = source_files("bg3-meta-cut", SAVE_INFO, globals.clone(), &META_LSF[..META_LSF.len() - 1]);
    let row = &get(&src, "/dev/v1/bg3/saves")["saves"][0];
    assert_eq!(row["fallbackReason"], json!("lsf-unreadable"), "{row}");
    assert_eq!(row["lsfVersions"], json!({ "meta": null, "globals": 7 }), "{row}");
    let (whole, _) = source_files("bg3-meta-whole", SAVE_INFO, globals, META_LSF);
    let row = &get(&whole, "/dev/v1/bg3/saves")["saves"][0];
    assert_eq!(row["lsfVersions"]["meta"], json!(7), "an intact meta.lsf keeps its version: {row}");
}

/// Codex P2: a pinned version that differs is layout drift even at the
/// pinned size; an unpinned version accepts any value.
#[test]
fn a_pinned_version_mismatch_is_unsupported_at_the_same_size() {
    use crate::dev_bridge::bg3_newage::DecoderStatus;
    use crate::dev_bridge::bg3_newage_reader::{status_in, Layout};
    let pinned = [Layout { name: "game.stats.v3.StatsComponent", version: Some(2), element_size: Some(36) }];
    let s = |v, size| status_in(&pinned, "game.stats.v3.StatsComponent", v, size);
    assert_eq!(s(2, 36), DecoderStatus::Ok);
    assert_eq!(s(3, 36), DecoderStatus::Unsupported, "same size, other version");
    assert_eq!(s(2, 40), DecoderStatus::Unsupported);
    assert_eq!(status_in(&pinned, "game.v0.Other", 2, 36), DecoderStatus::None);
    let unpinned = [Layout { name: "game.stats.v3.StatsComponent", version: None, element_size: Some(36) }];
    assert_eq!(status_in(&unpinned, "game.stats.v3.StatsComponent", 9, 36), DecoderStatus::Ok);
}
