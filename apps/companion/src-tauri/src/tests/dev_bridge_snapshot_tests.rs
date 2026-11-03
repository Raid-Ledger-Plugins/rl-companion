//! ROK-2072 PR-C: dev snapshot validation, dedupe, prune and the cache's
//! on-disk layout. Synthetic snapshots only (fake GUIDs, no names).

use super::temp_dir;
use crate::dev_bridge::snapshot_cache::{
    character_key, digest, merge, prune_rows, Row, SnapshotCache, MAX_ROWS_PER_FILE, RETAIN_SECS,
};
use crate::dev_bridge::snapshot_validate::{validate_snapshot, SNAPSHOT_MAX_BYTES};
use crate::dev_bridge::token::sha256_hex;
use rl_collectors::DevSnapshotEntry;
use serde_json::{json, Value};

const G1: &str = "Player-0000-0A000001";
const FLAVOR: &str = "_classic_era_";
const NOW: i64 = 1_793_000_000; // 2026-10-26T…Z

fn snap(captured: &str, build: &str) -> Value {
    json!({
        "snapshotVersion": 1, "addonVersion": "1.2.3", "capturedAt": captured, "trigger": "logout",
        "client": { "version": "1.60.1", "build": build, "buildDate": "Nov 1 2026", "interface": 11600,
                    "tocInterface": 11600, "locale": "enUS", "region": 1 },
        "character": { "class": "MAGE", "level": 60, "faction": "Horde", "inGuild": true },
        "ruleset": { "isGameRuleActive": "function", "enumGameRule": "table",
                     "rules": [{ "name": "SyntheticRuleA", "value": 1, "active": true }],
                     "rulesTruncated": false, "ledgerLink": { "value": null, "source": null } },
        "calls": [{ "fn": "GetBuildInfo", "status": "ok", "values": ["1.60.1", 5] },
                  { "fn": "UnitGUID", "status": "presence", "type": "function" }],
        "apis": [{ "path": "C_SkillInfo", "type": "nil" }, { "path": "GetSkillLineInfo", "type": "nil" }],
        "sections": [{ "id": "S9", "status": "ok", "lines": ["professions: none"] }],
        "truncated": false
    })
}

fn ts(secs: i64) -> String {
    rl_collectors::rfc3339_utc_from_unix_seconds(secs)
}

fn entry(v: &Value) -> DevSnapshotEntry {
    DevSnapshotEntry {
        guid: G1.into(),
        json: v.to_string(),
    }
}

#[test]
fn a_well_formed_snapshot_validates() {
    validate_snapshot(&snap(&ts(NOW), "1")).expect("valid");
}

#[test]
fn rejects_guid_unknown_key_presence_values_duplicates_and_version() {
    type Mutate = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Mutate)> = vec![
        (
            "sections.0.lines.0",
            Box::new(|v| v["sections"][0]["lines"][0] = json!("Player-1234-0ABCDEF0")),
        ),
        (
            "character",
            Box::new(|v| v["character"]["name"] = json!("x")),
        ),
        (
            "calls.1",
            Box::new(|v| v["calls"][1]["values"] = json!([1])),
        ),
        (
            "calls.0",
            Box::new(|v| v["calls"][0]["type"] = json!("function")),
        ),
        (
            "apis.1",
            Box::new(|v| v["apis"][1]["path"] = json!("C_SkillInfo")),
        ),
        (
            "snapshotVersion",
            Box::new(|v| v["snapshotVersion"] = json!(2)),
        ),
        (
            "sections.0",
            Box::new(|v| v["sections"][0]["lines"] = json!(vec!["y".repeat(200); 20])),
        ),
    ];
    for (at, mutate) in cases {
        let mut v = snap(&ts(NOW), "1");
        mutate(&mut v);
        let e = validate_snapshot(&v).expect_err(at);
        assert!(
            e.starts_with(&format!("{at}:")),
            "expected an issue at {at}, got {e}"
        );
    }
}

#[test]
fn whole_snapshot_over_48_kib_is_rejected_at_root() {
    let mut v = snap(&ts(NOW), "1");
    // Every item valid on its own (lines ≤ 256 chars, each item ≤ 3 KiB).
    let line = json!({ "id": "S3", "status": "ok", "lines": vec!["z".repeat(250); 11] });
    let call = |i: usize| {
        json!({ "fn": format!("F{i}"), "status": "error", "error": "e".repeat(500),
                "values": vec!["v".repeat(400); 2] })
    };
    v["sections"] = json!(vec![line; 9]);
    v["calls"] = json!((0..14).map(call).collect::<Vec<_>>());
    let len = serde_json::to_vec(&v).unwrap().len();
    assert!(len <= SNAPSHOT_MAX_BYTES, "{len}");
    if let Err(e) = validate_snapshot(&v) {
        panic!("under the cap ({len} bytes) but rejected: {e}");
    }
    v["calls"] = json!((0..30).map(call).collect::<Vec<_>>());
    let len = serde_json::to_vec(&v).unwrap().len();
    assert!(len > SNAPSHOT_MAX_BYTES, "{len}");
    let e = validate_snapshot(&v).expect_err("over the cap");
    assert!(e.starts_with("root: snapshot over 48 KiB"), "{e}");
}

#[test]
fn character_key_is_the_sv_parsed_guid_hash() {
    assert_eq!(character_key(G1), sha256_hex(G1));
    assert_eq!(character_key(G1).len(), 64);
}

#[test]
fn digest_ignores_captured_at_only() {
    assert_eq!(
        digest(&snap(&ts(NOW), "1")),
        digest(&snap(&ts(NOW + 60), "1"))
    );
    assert_ne!(digest(&snap(&ts(NOW), "1")), digest(&snap(&ts(NOW), "2")));
}

#[test]
fn dedupe_bumps_seen_count_and_a_b_a_keeps_three_rows() {
    let mut rows: Vec<Row> = Vec::new();
    assert!(merge(&mut rows, snap(&ts(NOW), "1"), NOW));
    assert!(
        !merge(&mut rows, snap(&ts(NOW), "1"), NOW),
        "same capture is a no-op"
    );
    assert!(merge(&mut rows, snap(&ts(NOW + 60), "1"), NOW + 60));
    assert_eq!((rows.len(), rows[0].seen_count), (1, 2));
    assert_eq!(rows[0].last_captured_at, ts(NOW + 60));
    assert!(merge(&mut rows, snap(&ts(NOW + 120), "2"), NOW + 120));
    assert!(merge(&mut rows, snap(&ts(NOW + 180), "1"), NOW + 180));
    assert_eq!(rows.len(), 3);
    assert_ne!(rows[0].snapshot_id, rows[2].snapshot_id);
    assert!(rows.iter().all(|r| r.snapshot_id.len() == 26));
}

#[test]
fn prune_drops_old_rows_but_keeps_the_latest_and_caps_rows() {
    let old = NOW - RETAIN_SECS - 86_400;
    let mut rows = Vec::new();
    merge(&mut rows, snap(&ts(old), "1"), old);
    merge(&mut rows, snap(&ts(old + 1), "2"), old + 1);
    prune_rows(&mut rows, NOW);
    assert_eq!(rows.len(), 1, "8-day-old latest kept, older one dropped");
    assert_eq!(rows[0].last_captured_at, ts(old + 1));
    let mut many = Vec::new();
    for i in 0..(MAX_ROWS_PER_FILE as i64 + 5) {
        merge(&mut many, snap(&ts(NOW + i), &i.to_string()), NOW + i);
    }
    prune_rows(&mut many, NOW + 300);
    assert_eq!(many.len(), MAX_ROWS_PER_FILE);
    assert_eq!(
        many.last().unwrap().captured_at,
        ts(NOW + MAX_ROWS_PER_FILE as i64 + 4)
    );
}

#[test]
fn ingest_writes_only_under_the_cache_root_and_skips_invalid() {
    let dir = temp_dir("snapshot-cache-layout");
    let cache = SnapshotCache::in_app_data(&dir);
    let bad = DevSnapshotEntry {
        guid: G1.into(),
        json: "{\"snapshotVersion\":2}".into(),
    };
    assert_eq!(cache.ingest(FLAVOR, &[bad], NOW), 0);
    assert_eq!(cache.ingest(FLAVOR, &[entry(&snap(&ts(NOW), "1"))], NOW), 1);
    assert_eq!(
        cache.ingest(FLAVOR, &[entry(&snap(&ts(NOW), "1"))], NOW),
        0,
        "re-read is a no-op"
    );
    let key = character_key(G1);
    let file = dir
        .join("dev-bridge")
        .join("snapshots")
        .join(FLAVOR)
        .join(format!("{key}.jsonl"));
    assert!(file.is_file(), "{}", file.display());
    assert_eq!(cache.characters(FLAVOR), std::slice::from_ref(&key));
    let rows = cache.rows(FLAVOR, &key);
    assert_eq!((rows.len(), rows[0].seen_count), (1, 1));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains(G1), "the GUID never reaches the cache");
    assert_eq!(
        cache.ingest("../etc", &[entry(&snap(&ts(NOW + 5), "9"))], NOW),
        0
    );
    assert!(!dir.join("dev-bridge").join("etc").exists());
}

/// Comments stripped first, so this file's own prose cannot trip the guard.
#[test]
fn the_cache_has_no_path_to_the_uploader() {
    for src in [
        include_str!("../dev_bridge/snapshot_cache.rs"),
        include_str!("../dev_bridge/snapshot_validate.rs"),
    ] {
        let code: String = src
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in ["upload", "blob_send", "reqwest", "runtime::"] {
            assert!(!code.contains(banned), "cache code references {banned}");
        }
    }
}

fn pinned(rel: &str) -> Value {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../contracts/dev-bridge/v1/fixtures")
        .join(rel);
    serde_json::from_str(&std::fs::read_to_string(p).expect(rel)).expect(rel)
}

#[test]
fn a_guid_as_key_snapshot_is_rejected_without_the_guid_reaching_the_log() {
    let v = pinned("invalid/ledgerlink-snapshot--guid-as-key.json");
    let e = validate_snapshot(&v).expect_err("guid-as-key");
    assert!(
        !e.contains("Player-"),
        "validator error echoes the GUID: {e}"
    );
    assert!(e.starts_with("calls.4.values.0.<guid>:"), "{e}");

    let cache = SnapshotCache::in_app_data(&temp_dir("snapshot-guid-log"));
    assert_eq!(cache.ingest(FLAVOR, &[entry(&v)], NOW), 0);
    let lines = crate::dev_bridge::logbuf::global().select(0, None, None);
    let msgs: Vec<String> = lines.iter().map(|(_, l)| l["msg"].to_string()).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("rejected") && m.contains("<guid>")),
        "the rejection is logged: {msgs:?}"
    );
    let leaked: Vec<&String> = msgs.iter().filter(|m| m.contains("Player-")).collect();
    assert!(
        leaked.is_empty(),
        "a GUID reached the LogBuffer: {leaked:?}"
    );
}

#[test]
fn the_log_redacts_player_guids() {
    let out = crate::dev_bridge::logbuf::redact("bad key Player-1234-0ABCDEF0 at calls.4");
    assert_eq!(out, "bad key <guid> at calls.4");
}

#[test]
fn a_probe_value_table_holds_at_most_40_keys() {
    let table = |n: usize| -> Value {
        Value::Object((0..n).map(|i| (format!("k{i}"), json!(i))).collect())
    };
    let mut v = snap(&ts(NOW), "1");
    v["calls"][0]["values"] = json!([table(40)]);
    validate_snapshot(&v).expect("40 keys is the cap");
    v["calls"][0]["values"] = json!([table(41)]);
    let e = validate_snapshot(&v).expect_err("41 keys must be rejected");
    assert!(
        e.starts_with("calls.0:"),
        "expected an issue at calls.0, got {e}"
    );
}

#[test]
fn string_caps_count_utf16_units_and_region_is_a_safe_integer() {
    let mut v = snap(&ts(NOW), "1");
    // 128 emoji = 128 chars but 256 UTF-16 units: exactly the 256 cap.
    v["sections"][0]["lines"] = json!(["😀".repeat(128)]);
    validate_snapshot(&v).expect("256 UTF-16 units is the cap");
    // 129 emoji = 129 chars (under 256) but 258 UTF-16 units, as Zod counts.
    v["sections"][0]["lines"] = json!(["😀".repeat(129)]);
    let e = validate_snapshot(&v).expect_err("258 UTF-16 units must be rejected");
    assert!(e.starts_with("sections.0:"), "got {e}");

    let mut v = snap(&ts(NOW), "1");
    v["client"]["region"] = json!(9_007_199_254_740_991_i64);
    validate_snapshot(&v).expect("2^53 - 1 is a safe integer");
    v["client"]["region"] = json!(9_007_199_254_740_992_i64);
    let e = validate_snapshot(&v).expect_err("2^53 is not a safe integer");
    assert!(e.starts_with("client:"), "got {e}");
}

#[test]
fn rows_written_in_the_same_second_get_increasing_ids() {
    let mut rows: Vec<Row> = Vec::new();
    for b in ["1", "2", "3", "4", "5", "6"] {
        assert!(merge(&mut rows, snap(&ts(NOW), b), NOW), "{b} appends");
    }
    let ids: Vec<&str> = rows.iter().map(|r| r.snapshot_id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, ids, "ids sort in write order");
}
