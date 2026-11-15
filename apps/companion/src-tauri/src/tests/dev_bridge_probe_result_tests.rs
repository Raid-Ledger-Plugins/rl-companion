//! `probe-result` validation + paging (ROK-2191) against the pinned
//! dev-bridge/v1 fixtures and synthetic over-cap results.

use super::dev_bridge_unit_tests::schema_errors;
use crate::dev_bridge::paging::json_len;
use crate::dev_bridge::probe_result::{validate_result, PROBE_RESULT_MAX_BYTES};
use crate::dev_bridge::probe_result_page::{assemble_page, page_items};
use serde_json::{json, Value};
use std::path::PathBuf;

const ID: &str = "01JBX7Q2M3N4P5R6S7T8V9W0XY";
const ID2: &str = "01JBX7Q2M3N4P5R6S7T8V9W0XZ";

fn fixtures(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../contracts/dev-bridge/v1/fixtures")
        .join(sub)
}

/// `(name, text)` of every `probe-result--*.json` in `sub`.
fn probe_result_fixtures(sub: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(fixtures(sub))
        .expect("fixtures dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("probe-result--") && n.ends_with(".json"))
        .map(|n| {
            let text = std::fs::read_to_string(fixtures(sub).join(&n)).expect("fixture");
            (n, text)
        })
        .collect();
    out.sort();
    out
}

fn result(calls: Value) -> Value {
    json!({ "probeId": ID, "capturedAt": "2026-11-01T19:02:11Z",
            "addonVersion": "0.1.0", "truncated": false, "calls": calls })
}

fn check(v: &Value) -> Result<Value, String> {
    validate_result(&v.to_string(), ID)
}

#[test]
fn every_valid_probe_result_fixture_passes_rust_and_schema() {
    let all = probe_result_fixtures("");
    assert!(all.len() >= 2, "fixtures found: {}", all.len());
    for (name, text) in all {
        let v = validate_result(&text, ID).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            schema_errors("probe-result", &v),
            Vec::<String>::new(),
            "{name}"
        );
    }
}

#[test]
fn every_invalid_probe_result_fixture_is_rejected_at_its_path() {
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures("invalid/EXPECTED.json")).expect("EXPECTED"),
    )
    .expect("EXPECTED parses");
    let all = probe_result_fixtures("invalid");
    assert!(all.len() >= 3, "invalid fixtures found: {}", all.len());
    for (name, text) in all {
        let reason = validate_result(&text, ID).expect_err(&name);
        let key = name.trim_end_matches(".json");
        let path = expected[key]["path"].as_str().expect("EXPECTED path");
        assert!(reason.starts_with(&format!("{path}:")), "{name}: {reason}");
    }
}

#[test]
fn answered_probe_detail_result_passes() {
    let text = std::fs::read_to_string(fixtures("probe-detail--answered.json")).expect("fixture");
    let detail: Value = serde_json::from_str(&text).expect("parses");
    let v = check(&detail["result"]).expect("valid");
    assert_eq!(v, detail["result"]);
}

#[test]
fn probe_id_must_match_the_sv_key() {
    let v = result(json!([]));
    let reason = validate_result(&v.to_string(), ID2).expect_err("mismatch");
    assert!(reason.starts_with("probeId:"), "{reason}");
}

#[test]
fn shape_rules_reject_without_echoing_values() {
    let cases = [
        (
            json!([{ "id": "a", "status": "ok", "secretKey": 1 }]),
            "calls.0:",
        ),
        (
            json!([{ "id": "a", "status": "ok", "truncated": false }]),
            "calls.0.truncated:",
        ),
        (json!([{ "id": "1a", "status": "ok" }]), "calls.0.id:"),
        (
            json!([{ "id": "a", "status": "ok", "iterations": [{ "i": 0, "status": "ok" }] }]),
            "calls.0.iterations.0.i:",
        ),
        (
            json!([{ "id": "a", "status": "ok", "error": "e".repeat(513) }]),
            "calls.0.error:",
        ),
        (
            json!([{ "id": "a", "status": "ok", "values": [{ "k".repeat(129): 1 }] }]),
            "calls.0.values.0.#0:",
        ),
    ];
    for (calls, path) in cases {
        let reason = check(&result(calls)).expect_err(path);
        assert!(reason.starts_with(path), "{path} → {reason}");
        assert!(
            !reason.contains("secretKey") && !reason.contains("kkk"),
            "{reason}"
        );
    }
    let mut bad_ts = result(json!([]));
    bad_ts["capturedAt"] = json!("2026-11-01T19:02:11+01:00");
    assert!(check(&bad_ts).expect_err("ts").starts_with("capturedAt:"));
    let mut bad_ver = result(json!([]));
    bad_ver["addonVersion"] = json!("v1");
    assert!(check(&bad_ver)
        .expect_err("ver")
        .starts_with("addonVersion:"));
}

#[test]
fn depth_entry_and_size_caps_are_enforced() {
    let deep = json!([{ "id": "a", "status": "ok", "values": [[[[[1]]]]] }]);
    assert!(check(&result(deep)).expect_err("depth").contains("3 deep"));
    let three = json!([{ "id": "a", "status": "ok", "values": [[[[1]]]] }]);
    check(&result(three)).expect("depth 3 is fine");
    let wide = json!([{ "id": "a", "status": "ok", "values": [vec![1; 41]] }]);
    assert!(check(&result(wide)).expect_err("41").contains("40 entries"));
    let many = json!([{ "id": "a", "status": "ok", "values": vec![1; 41] }]);
    assert!(check(&result(many))
        .expect_err("41 values")
        .starts_with("calls.0.values:"));
    let big = json!([{ "id": "a", "status": "ok", "values": vec!["v".repeat(500); 7] }]);
    assert!(check(&result(big))
        .expect_err("item")
        .starts_with("calls.0: result item"));
    let it = json!({ "i": 1, "status": "ok", "values": vec!["v".repeat(500); 7] });
    let big_it = json!([{ "id": "a", "status": "ok", "iterations": [it] }]);
    assert!(check(&result(big_it))
        .expect_err("iter")
        .starts_with("calls.0.iterations.0:"));
}

#[test]
fn whole_result_over_64_kib_is_rejected() {
    let it = |i: u64| json!({ "i": i, "status": "ok", "values": vec!["v".repeat(500); 5] });
    let call = |c: usize| {
        json!({ "id": format!("c{c}"), "status": "ok",
                                  "iterations": (1..=20).map(it).collect::<Vec<_>>() })
    };
    let v = result(Value::Array((0..2).map(call).collect()));
    assert!(json_len(&v) > PROBE_RESULT_MAX_BYTES);
    let reason = check(&v).expect_err("whole");
    assert!(reason.starts_with("result: is"), "{reason}");
}

/// Merge pages the way a client does: calls by `id`, iterations appended.
fn merge(pages: &[Value]) -> Value {
    let mut out = pages[0].clone();
    let mut calls: Vec<Value> = Vec::new();
    for p in pages {
        for c in p["calls"].as_array().expect("calls") {
            match calls.iter_mut().find(|x| x["id"] == c["id"]) {
                Some(x) => {
                    let more = c["iterations"].as_array().cloned().unwrap_or_default();
                    x["iterations"].as_array_mut().expect("its").extend(more);
                }
                None => calls.push(c.clone()),
            }
        }
    }
    out["calls"] = Value::Array(calls);
    out
}

fn pages(v: &Value, budget: usize) -> Vec<Value> {
    let items = page_items(v);
    let (mut out, mut at) = (Vec::new(), Some(0));
    while let Some(start) = at {
        let (page, next) = assemble_page(v, &items, start, budget);
        assert!(
            json_len(&page) <= budget,
            "page {} > {budget}",
            json_len(&page)
        );
        out.push(page);
        at = next;
    }
    out
}

#[test]
fn paging_splits_items_and_round_trips() {
    let text = std::fs::read_to_string(fixtures("probe-detail--answered.json")).expect("fixture");
    let v: Value = serde_json::from_str::<Value>(&text).expect("parses")["result"].clone();
    let items = page_items(&v);
    let kinds: Vec<(&str, Option<u64>)> = items.iter().map(|i| (i.id.as_str(), i.i)).collect();
    assert_eq!(
        kinds,
        [
            ("profs", None),
            ("n", None),
            ("skill", None),
            ("skill", Some(1)),
            ("skill", Some(2)),
            ("ts", None)
        ]
    );
    let (one, next) = assemble_page(&v, &items, 0, 16_384);
    assert_eq!((one, next), (v.clone(), None));
    let it = |i: u64| json!({ "i": i, "status": "ok", "values": vec!["v".repeat(400); 2] });
    let big = result(json!([{ "id": "n", "status": "ok", "values": [60] },
        { "id": "loop", "status": "ok", "truncated": true, "iterations": (1..=60).map(it).collect::<Vec<_>>() },
        { "id": "empty", "status": "ok", "iterations": [] }]));
    check(&big).expect("valid");
    let paged = pages(&big, 16_384);
    assert!(paged.len() >= 3, "pages: {}", paged.len());
    assert!(paged[1]["calls"][0]["id"] == "loop" && paged[1]["calls"][0]["truncated"] == true);
    assert_eq!(merge(&paged), big);
}

/// `s` as a double-quoted Lua string literal, the way WoW writes SavedVariables.
fn lua_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[test]
fn savedvariables_round_trip_one_valid_one_invalid() {
    let good =
        std::fs::read_to_string(fixtures("probe-result--nested-tables.json")).expect("fixture");
    let good: Value = serde_json::from_str(&good).expect("parses");
    let mut bad = good.clone();
    bad["probeId"] = json!(ID2);
    bad["calls"][0]["status"] = json!("exploded");
    let lua = format!(
        "LedgerLinkDB = {{\n[\"dev\"] = {{\n[\"probes\"] = {{\n[\"{ID}\"] = {},\n[\"{ID2}\"] = {},\n}},\n}},\n}}\n",
        lua_str(&good.to_string()),
        lua_str(&bad.to_string())
    );
    let probes = rl_collectors::parse_ledgerlink_dev_probes(lua.as_bytes()).expect("parses");
    assert!(probes.skipped.is_empty(), "{:?}", probes.skipped);
    let verdicts: Vec<(String, Result<Value, String>)> = probes
        .results
        .iter()
        .map(|(id, raw)| (id.clone(), validate_result(raw, id)))
        .collect();
    assert_eq!(verdicts.len(), 2);
    assert_eq!(verdicts[0], (ID.to_string(), Ok(good)));
    let reason = verdicts[1].1.clone().expect_err("invalid probe");
    assert!(reason.starts_with("calls.0.status:"), "{reason}");
    assert!(!reason.contains("exploded"), "{reason}");
}
