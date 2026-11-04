//! `/dev/v1/bg3/*` (ROK-2078 layer 1): saveRef, the BG3 cursor, redaction
//! against leaky values, and every page valid against the pinned schema.

use super::bg3_collector_tests::lspk;
use super::dev_bridge_unit_tests::schema_errors;
use super::temp_dir;
use crate::dev_bridge::bg3_newage::*;
use crate::dev_bridge::bg3_redact::{body_is_clean, class_id, holds_guid, holds_long_digits, stats_id};
use crate::dev_bridge::bg3_routes::{self, decode, encode, Cursor};
use crate::dev_bridge::bg3_saves::{hmac_sha256, load_or_create_key_with, save_ref, Bg3Source, KEY_FILE};
use crate::dev_bridge::paging::{parse_query, PAGE_MAX_BYTES};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const SAVE_INFO: &[u8] =
    include_bytes!("../../../../../contracts/bg3-save/v1/fixtures/saveinfo/act2-party-4p-summon.saveinfo.json");
const META_LSF: &[u8] =
    include_bytes!("../../../../../crates/rl-collectors/tests/fixtures/meta/real-shape-string.meta.lsf");
/// A folder name carrying a 10-digit run: it must never reach a body.
const FOLDER: &str = "QuickSave_1-1234567890";
const KEY: [u8; 32] = [7; 32];
const STEAM: &str = "76561198012345678";
const GUID_US: &str = "1a2b3c4d_5e6f_7a8b_9c0d_1e2f3a4b5c6d";

fn install(story: &Path, folder: &str, with_meta: bool) {
    let dir = story.join(folder);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut files: Vec<(&str, &[u8])> = vec![("SaveInfo.json", SAVE_INFO), ("Globals.lsf", b"LSOF")];
    if with_meta {
        files.push(("meta.lsf", META_LSF));
    }
    std::fs::write(dir.join("QuickSave_1.lsv"), lspk(&files)).expect("write lsv");
}

fn call(src: &Bg3Source, path: &str, query: &str) -> (u16, Value) {
    bg3_routes::get(Some(src), path, &parse_query(Some(query))).expect("bg3 route")
}

/// Every page from the first: 200, schema-valid, ≤ 16 KiB, clean.
fn pages(src: &Bg3Source, path: &str, def: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut q = String::new();
    for _ in 0..50 {
        let (status, body) = call(src, path, &q);
        assert_eq!(status, 200, "{path}?{q} → {body}");
        assert_eq!(schema_errors(def, &body), Vec::<String>::new(), "{def} page {}", out.len() + 1);
        let text = body.to_string();
        assert!(text.len() <= PAGE_MAX_BYTES, "page is {} bytes", text.len());
        for leak in [FOLDER, "1234567890", STEAM, "Astarion", "/Users/", GUID_US] {
            assert!(!text.contains(leak), "body leaks {leak:?}: {text}");
        }
        let next = body["next"].as_str().map(str::to_owned);
        out.push(body);
        match next {
            Some(n) => q = format!("cursor={n}"),
            None => return out,
        }
    }
    panic!("paging did not end");
}

#[test]
fn hmac_matches_rfc4231_and_save_ref_is_12_hex() {
    let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    let root = Path::new("/story");
    let r = save_ref(&KEY, root, &format!("{FOLDER}/QuickSave_1.lsv"));
    assert_eq!(r.len(), 12);
    assert!(r.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{r}");
    assert_ne!(r, save_ref(&[8; 32], root, &format!("{FOLDER}/QuickSave_1.lsv")), "keyed");
}

#[test]
fn cursor_round_trips_and_rejects_names() {
    for c in ["p1_saves_0", "p2_types_9", "p9999_party_999999"] {
        assert_eq!(decode(c).map(|x| encode(&x)).as_deref(), Some(c));
    }
    let c = Cursor { page: 3, section: "party".into(), offset: 2 };
    assert_eq!(decode(&encode(&c)), Some(c));
    for bad in ["p0_types_1", "p1_names_0", "p1_types_1234567", FOLDER, "p1_saves_0/..", ""] {
        assert_eq!(decode(bad), None, "{bad:?} must not decode");
    }
}

#[test]
fn redaction_drops_leaky_values() {
    assert_eq!(stats_id("ARM_Hat").as_deref(), Some("ARM_Hat"));
    for v in ["Astarion", "Shadowheart_Robe", "/Users/p/ARM_Hat", "WPN_76561198012345678", GUID_US] {
        assert_eq!(stats_id(v), None, "statsId {v:?}");
    }
    assert_eq!(class_id("Wizard").as_deref(), Some("Wizard"));
    for v in ["6f2b8c1e", GUID_US, "Evoc-School", "a b", "C:\\x", STEAM] {
        assert_eq!(class_id(v), None, "classId {v:?}");
    }
    for g in [GUID_US, "1a2b3c4d__5e6f__7a8b__9c0d__1e2f3a4b5c6d", "{1A2B3C4D-5E6F-7A8B-9C0D-1E2F3A4B5C6D}",
        "H1A2B3C4DG5E6FG7A8BG9C0DG1E2F3A4B5C6D"] {
        assert!(holds_guid(g), "{g} is a GUID");
    }
    assert!(holds_long_digits(STEAM));
    assert!(!body_is_clean(&json!({ "x": [{ "classId": STEAM }] })));
    assert!(!body_is_clean(&json!({ GUID_US: 1 })));
    assert!(body_is_clean(&json!({ "saveRef": "123456789012", "gameVersion": "4.1.1.6758295" })));
}

#[test]
fn saves_listing_is_schema_valid_and_never_names_the_save() {
    let story = temp_dir("bg3-saves-list");
    install(&story, FOLDER, true);
    install(&story, "Tav-9876543210", false);
    let src = Bg3Source::with_key(vec![story.clone()], KEY, Arc::new(NotYetDecoded));
    let all = pages(&src, "/dev/v1/bg3/saves", "bg3-saves");
    let rows: Vec<&Value> = all.iter().flat_map(|p| p["saves"].as_array().into_iter().flatten()).collect();
    assert_eq!(rows.len(), 2);
    let reasons: Vec<&Value> = rows.iter().map(|r| &r["fallbackReason"]).collect();
    assert!(reasons.contains(&&json!("meta-missing")), "{reasons:?}");
    assert!(rows.iter().all(|r| r["partySize"] == json!(5) && r["gameVersion"] == json!("4.1.1.7631656")));
    assert!(!all[0].to_string().contains("Tav"), "folder name leaked");
    let (status, _) = call(&src, "/dev/v1/bg3/saves", "cursor=p2_saves_32");
    assert_eq!(status, 404, "a cursor past the listing restarts");
    assert_eq!(call(&src, "/dev/v1/bg3/saves", "cursor=QuickSave_1").0, 400);
}

/// A lane-B stand-in that returns many types and leaky decoded values.
struct Leaky;
impl NewAgeDecoder for Leaky {
    fn read(&self, _g: &[u8], party: usize) -> NewAgeOutcome {
        let mut types: Vec<TypeRow> = (0..400)
            .map(|i| TypeRow { type_name: format!("game.stats.v{i}.StatsComponent"), version: 3,
                element_size: 36, count: 2310, decoder: DecoderStatus::Ok })
            .collect();
        types.push(TypeRow { type_name: format!("x.{GUID_US}.y"), version: 0, element_size: 8, count: 1,
            decoder: DecoderStatus::None });
        let member = PartyDecoded {
            abilities: Some([17, 14, 15, 8, 12, 10]),
            proficiency_bonus: Some(3),
            max_hp: Some(52),
            class_levels: Some(vec![
                ClassLevel { class_id: "Fighter".into(), subclass_id: Some(GUID_US.into()), level: 6 },
                ClassLevel { class_id: GUID_US.into(), subclass_id: None, level: 2 },
            ]),
            equipment: Some(vec![("Helmet".into(), "ARM_Hat".into()), ("Breast".into(), "Shadowheart_Robe".into()),
                ("Helmet".into(), "ARM_Dup".into()), ("Pocket".into(), "ARM_X".into())]),
        };
        NewAgeOutcome::Read(NewAgeRead { types, checks: Checks::default(), party: vec![member; party] })
    }
}

#[test]
fn structure_pages_types_then_party_and_redacts_decoded_values() {
    let story = temp_dir("bg3-structure");
    install(&story, FOLDER, true);
    let src = Bg3Source::with_key(vec![story.clone()], KEY, Arc::new(Leaky));
    let r = save_ref(&KEY, &story, &format!("{FOLDER}/QuickSave_1.lsv"));
    let path = format!("/dev/v1/bg3/saves/{r}/structure");
    let all = pages(&src, &path, "bg3-save-structure");
    assert!(all.len() > 1, "400 type rows need more than one page");
    let types: usize = all.iter().map(|p| p["types"].as_array().map_or(0, Vec::len)).sum();
    assert_eq!(types, 400, "the GUID-named type row is dropped");
    let party: Vec<&Value> = all.iter().flat_map(|p| p["party"].as_array().into_iter().flatten()).collect();
    assert_eq!(party.len(), 5);
    let d = &party[0]["decoded"];
    assert_eq!(d["classLevels"], json!([{ "classId": "Fighter", "subclassId": null, "level": 6 }]));
    assert_eq!(d["equipment"], json!([{ "slot": "Helmet", "statsId": "ARM_Hat" }, { "slot": "Breast", "statsId": null }]));
    let first_next = all[0]["next"].as_str().expect("page 1 has next").to_owned();
    let shifted = first_next.replace("p2_types_", "p2_types_1");
    assert_eq!(call(&src, &path, &format!("cursor={shifted}")).0, 404, "{shifted} does not start page 2");
    assert_eq!(call(&src, &path, "cursor=p2_party_0").0, 404);
    assert_eq!(call(&src, &path, "cursor=p1_types_0").0, 404, "page 1 is the bare request");
    assert_eq!(call(&src, "/dev/v1/bg3/saves/000000000000/structure", "").0, 404);
    assert_eq!(call(&src, &format!("/dev/v1/bg3/saves/{FOLDER}/structure"), "").0, 404);
}

#[test]
fn structure_not_yet_decoded_is_the_allowed_empty_form() {
    let story = temp_dir("bg3-structure-empty");
    install(&story, FOLDER, true);
    let src = Bg3Source::with_key(vec![story.clone()], KEY, Arc::new(NotYetDecoded));
    let r = save_ref(&KEY, &story, &format!("{FOLDER}/QuickSave_1.lsv"));
    let all = pages(&src, &format!("/dev/v1/bg3/saves/{r}/structure"), "bg3-save-structure");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["types"], json!([]));
    assert_eq!(all[0]["party"][0], json!({ "slot": 0, "decoded": {} }));
    assert_eq!(all[0]["checks"]["partyEntitiesFound"], json!(0));
}

/// Review MINOR: a failed RNG must not persist (or use) an all-zero key.
#[test]
fn a_failed_rng_refuses_rather_than_saving_a_zero_key() {
    let dir = temp_dir("bg3-key-rng-fail");
    assert_eq!(load_or_create_key_with(Some(&dir), |_| false), None);
    let path = KEY_FILE.iter().fold(dir.clone(), |p, s| p.join(s));
    assert!(!path.exists(), "no key file is written when the RNG fails");
    let src = Bg3Source::without_key(vec![dir], Arc::new(NotYetDecoded));
    let (status, _) = bg3_routes::get(Some(&src), "/dev/v1/bg3/saves", &parse_query(Some(""))).expect("bg3 route");
    assert_eq!(status, 500, "no key → the bg3 routes refuse");
}

#[test]
fn the_key_file_is_written_once_owner_only_and_reused() {
    let dir = temp_dir("bg3-key-file");
    let first = load_or_create_key_with(Some(&dir), |k| { k.fill(0x42); true }).expect("key");
    let path = KEY_FILE.iter().fold(dir.clone(), |p, s| p.join(s));
    assert_eq!(std::fs::read_to_string(&path).expect("written"), "42".repeat(32));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the saveRef key is owner-only");
    }
    let again = load_or_create_key_with(Some(&dir), |_| panic!("an existing key is reused, not regenerated"));
    assert_eq!(again, Some(first));
}

/// Codex P2: the same folder/file pair under two roots gets two refs, both
/// listed and both findable; a root configured twice lists its file once.
#[test]
fn the_same_save_under_two_roots_gets_two_refs() {
    let (a, b) = (temp_dir("bg3-root-a"), temp_dir("bg3-root-b"));
    install(&a, FOLDER, true);
    install(&b, FOLDER, true);
    let src = Bg3Source::with_key(vec![a.clone(), b.clone(), a.clone()], KEY, Arc::new(NotYetDecoded));
    let all = pages(&src, "/dev/v1/bg3/saves", "bg3-saves");
    let refs: Vec<&str> = all[0]["saves"].as_array().expect("saves").iter().filter_map(|s| s["saveRef"].as_str()).collect();
    assert_eq!(refs.len(), 2, "one row per physical file: {refs:?}");
    assert_ne!(refs[0], refs[1], "same folder/file under two roots must not share a saveRef");
    for r in refs {
        let found = src.find(r).map(|(abs, _)| abs);
        assert!(found.is_some(), "{r} is findable");
    }
    let rel = format!("{FOLDER}/QuickSave_1.lsv");
    assert_eq!(src.find(&save_ref(&KEY, &b, &rel)).map(|f| f.0), Some(b.join(&rel)));
}
