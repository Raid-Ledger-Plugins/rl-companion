//! ROK-2191: the probe allowlist (every `allowlist.json` example vector) and
//! `POST /dev/v1/probes` body validation against the pinned fixtures — valid
//! requests parse, each invalid one fails at its `EXPECTED.json` path + code,
//! and the output form is a valid `$defs/probe-queue` entry.

use super::dev_bridge_unit_tests::schema_errors;
use crate::dev_bridge::paging::json_len;
use crate::dev_bridge::probe_allowlist::{allowlist, check, redact_rules, segment_words, Denial};
use crate::dev_bridge::probe_request::{
    error_body, validate, worst_bytes, PROBE_REQUEST_MAX_BYTES,
};
use serde_json::{json, Value};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/dev-bridge/v1/fixtures")
}

fn load(rel: &str) -> Value {
    let text = std::fs::read_to_string(fixtures().join(rel)).expect(rel);
    serde_json::from_str(&text).expect(rel)
}

fn stems(dir: &str, prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(fixtures().join(dir))
        .expect("fixture dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix) && n.ends_with(".json"))
        .map(|n| n.trim_end_matches(".json").to_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn allowlist_example_vectors_agree() {
    let ex = &allowlist().examples;
    for p in &ex.allowed {
        assert_eq!(check(p), Ok(()), "allowed: {p}");
    }
    for p in &ex.denied {
        assert!(check(p).is_err(), "denied: {p}");
    }
    for p in &ex.deny_listed {
        assert_eq!(check(p), Err(Denial::Denied), "denyListed: {p}");
    }
    for t in &ex.tokenizer {
        assert_eq!(
            segment_words(&t.segment),
            t.words,
            "tokenizer: {}",
            t.segment
        );
    }
    assert!(ex.deny_listed.len() >= allowlist().deny_words.len());
}

#[test]
fn allowlist_reasons_and_redact_table() {
    assert_eq!(check("A.B.C.GetMoney"), Err(Denial::Grammar));
    assert_eq!(check("Get..Money"), Err(Denial::Grammar));
    assert_eq!(check("UnitPopup_OnClick"), Err(Denial::NotReadVerb));
    assert_eq!(check("SomeAddon.time"), Err(Denial::NotReadVerb));
    assert_eq!(check("time"), Ok(()));
    let r = redact_rules();
    assert_eq!(r[0].fn_name, "GetGuildRosterInfo");
    assert_eq!(r[0].returns, vec![8]);
}

#[test]
fn valid_probe_requests_parse_and_output_form_is_a_queue_entry() {
    let names = stems(".", "probe-request--");
    assert!(names.len() >= 3, "{names:?}");
    for stem in names {
        let body = load(&format!("{stem}.json"));
        let req = validate(&body).unwrap_or_else(|e| panic!("{stem}: {e:?}"));
        let entry = req.queue_entry("01JBX7Q2M3N4P5R6S7T8V9W0XY", "_anniversary_");
        let queue = json!({ "queueVersion": 1, "probes": [entry] });
        assert_eq!(
            schema_errors("probe-queue", &queue),
            Vec::<String>::new(),
            "{stem}"
        );
        let echo = req.echo_json("_anniversary_");
        assert!(echo.get("probeId").is_none(), "{stem}");
        assert_eq!(echo["flavor"], "_anniversary_");
    }
}

#[test]
fn defaults_fill_the_output_form() {
    let req = validate(&load("probe-request--minimal.json")).expect("minimal");
    assert!(req.is_auto_flavor());
    assert_eq!(req.after, "PLAYER_LOGIN");
    assert_eq!(req.ttl_hours, 72);
    let out = req.to_output_json(None);
    assert_eq!(out["flavor"], "auto");
    assert_eq!(out["calls"][0], json!({ "id": "g", "fn": "IsInGuild" }));
}

#[test]
fn max_size_fits_exactly_and_over_size_fails() {
    let body = load("probe-request--max-size.json");
    let req = validate(&body).expect("max-size is valid");
    let entry = req.queue_entry(
        "01JBX7Q2M3N4P5R6S7T8V9W0XY",
        "_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_",
    );
    assert!(json_len(&entry) <= PROBE_REQUEST_MAX_BYTES);
    let out = req.to_output_json(None);
    assert!(worst_bytes(out.as_object().expect("object")) <= PROBE_REQUEST_MAX_BYTES);
    let max_entry = load("probe-queue--max-entry.json");
    assert_eq!(json_len(&max_entry["probes"][0]), PROBE_REQUEST_MAX_BYTES);
    let over = load("invalid/probe-request--over-size.json");
    let issues = validate(&over).expect_err("over-size");
    assert_eq!(
        (issues[0].dotted().as_str(), issues[0].code),
        ("", "custom")
    );
}

#[test]
fn invalid_probe_requests_fail_at_expected_path_and_code() {
    let expected = load("invalid/EXPECTED.json");
    let names = stems("invalid", "probe-request--");
    assert!(names.len() >= 20, "{names:?}");
    for stem in names {
        let issues = validate(&load(&format!("invalid/{stem}.json")))
            .expect_err(&format!("{stem}: accepted"));
        let want = &expected[&stem];
        let first = &issues[0];
        assert_eq!(
            first.dotted(),
            want["path"].as_str().expect("path"),
            "{stem}: {issues:?}"
        );
        assert_eq!(
            first.code,
            want["code"].as_str().expect("code"),
            "{stem}: {issues:?}"
        );
        let body = error_body(&issues);
        assert_eq!(
            schema_errors("error", &body),
            Vec::<String>::new(),
            "{stem}"
        );
    }
}

#[test]
fn zod_mirror_edge_cases() {
    let base = |calls: Value| json!({ "title": "t", "calls": calls });
    let first = |v: Value| {
        let i = validate(&v).expect_err("invalid");
        (i[0].dotted(), i[0].code)
    };
    let long = "x".repeat(129);
    let arg = base(json!([{ "id": "a", "fn": "GetMoney", "args": [long] }]));
    assert_eq!(first(arg), ("calls.0.args.0".into(), "too_big"));
    let flavor =
        json!({ "title": "t", "flavor": "AUTO", "calls": [{ "id": "a", "fn": "GetMoney" }] });
    assert_eq!(first(flavor), ("flavor".into(), "invalid_format"));
    let pres = base(json!([{ "id": "a", "presence": "C_Map", "args": [1] }]));
    assert_eq!(first(pres), ("calls.0.presence".into(), "custom"));
    let ttl = json!({ "title": "t", "ttlHours": 1.5, "calls": [{ "id": "a", "fn": "GetMoney" }] });
    assert_eq!(first(ttl), ("ttlHours".into(), "invalid_type"));
    let title = base(json!([{ "id": "a", "fn": "GetMoney" }]));
    let mut bidi = title.clone();
    bidi["title"] = json!("a\u{202E}b");
    assert_eq!(first(bidi), ("title".into(), "invalid_format"));
    let mut blank = title;
    blank["title"] = json!("   ");
    assert_eq!(first(blank), ("title".into(), "invalid_format"));
    let ok = base(json!([{ "id": "a", "fn": "GetMoney", "args": [2.0, true, null, "s"] }]));
    let req = validate(&ok).expect("literals");
    assert_eq!(req.to_output_json(None)["calls"][0]["args"][0], json!(2));
}

#[test]
fn error_body_caps_issue_count_and_bytes() {
    let calls: Vec<Value> = (0..40)
        .map(|i| json!({ "id": format!("c{i}"), "fn": "SendChatMessage", "args": ["$i"] }))
        .collect();
    let issues = validate(&json!({ "title": "t", "calls": calls })).expect_err("invalid");
    assert!(issues.len() > 50);
    let body = error_body(&issues);
    assert!(body["issues"].as_array().expect("issues").len() <= 50);
    assert!(json_len(&body) <= 16 * 1024);
}
