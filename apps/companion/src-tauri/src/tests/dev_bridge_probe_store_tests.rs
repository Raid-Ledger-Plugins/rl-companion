//! ROK-2191 probe store: DevQueue.lua rendering, flavor resolution, the
//! probe lifecycle and persistence.

use super::dev_bridge_unit_tests::schema_errors;
use crate::dev_bridge::probe_flavor::FlavorError;
use crate::dev_bridge::probe_id::{encode, is_probe_id, new_probe_id};
use crate::dev_bridge::probe_queue::{bracket_level, extract, render};
use crate::dev_bridge::probe_record::{ProbeRecord, ProbeStatus};
use crate::dev_bridge::probe_request::{validate, ProbeRequest};
use crate::dev_bridge::probe_store::{ProbeStore, ProbeStoreError};
use crate::dev_bridge::write_guard::{queue_path, WriteGuardError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const QUEUE_FIXTURE: &str =
    include_str!("../../../../../contracts/dev-bridge/v1/fixtures/probe-queue.json");
const NOW: i64 = 1_793_000_000;

fn request(flavor: &str, ttl: u32) -> ProbeRequest {
    let body = json!({ "title": "Can an addon read professions?", "flavor": flavor,
        "ttlHours": ttl, "calls": [{ "id": "profs", "fn": "GetProfessions" }] });
    validate(&body).expect("valid request")
}

/// A WoW folder with `flavors` installed (LedgerLink folder present).
fn wow(tag: &str, flavors: &[&str]) -> PathBuf {
    let dir = super::temp_dir(tag);
    for f in flavors {
        let q = queue_path(&dir, f).expect("flavor");
        std::fs::create_dir_all(q.parent().expect("parent")).expect("install");
    }
    dir
}

fn sv_at(wow: &Path, flavor: &str, age_secs: u64) {
    let dir = wow.join(flavor).join("WTF/Account/ACC/SavedVariables");
    std::fs::create_dir_all(&dir).expect("mkdir sv");
    let f = std::fs::File::create(dir.join("LedgerLink.lua")).expect("sv");
    let t = SystemTime::now() - Duration::from_secs(age_secs);
    f.set_modified(t).expect("mtime");
}

fn queue_of(wow: &Path, flavor: &str) -> Value {
    let text = std::fs::read_to_string(queue_path(wow, flavor).expect("p")).expect("read queue");
    let json = extract(&text).expect("one long-bracket string");
    serde_json::from_str(json).expect("queue JSON parses")
}

fn ids(queue: &Value) -> Vec<String> {
    let probes = queue["probes"].as_array().expect("probes");
    probes
        .iter()
        .map(|p| p["probeId"].as_str().unwrap_or("").to_owned())
        .collect()
}

fn list_of(rec: &ProbeRecord) -> Value {
    json!({ "probes": [rec.summary_json()], "next": null })
}

fn store(dir: &Path) -> ProbeStore {
    ProbeStore::in_app_data(dir)
}

#[test]
fn the_queue_fixture_round_trips_through_the_long_bracket() {
    let fixture: Value = serde_json::from_str(QUEUE_FIXTURE).expect("fixture");
    let probes = fixture["probes"].as_array().expect("probes").clone();
    let text = render(&probes);
    assert!(text.starts_with("RaidLedgerProbeQueue = [=[{"), "{text}");
    assert!(text.ends_with("}]=]\n"), "{text}");
    let back: Value = serde_json::from_str(extract(&text).expect("extract")).expect("parse");
    assert_eq!(back, fixture);
    assert_eq!(schema_errors("probe-queue", &back), Vec::<String>::new());
}

#[test]
fn the_bracket_level_escalates_past_every_closer_in_the_json() {
    assert_eq!(bracket_level(r#"{"a":"x"}"#), 0);
    assert_eq!(bracket_level(r#"{"a":"]]"}"#), 1);
    assert_eq!(bracket_level(r#"{"a":"]] ]=]"}"#), 2);
    assert_eq!(bracket_level(r#"{"a":"]] ]=] ]==]"}"#), 3);
    let entry = json!({ "a": "]] ]=] ]==]" });
    let text = render(std::slice::from_ref(&entry));
    assert!(text.starts_with("RaidLedgerProbeQueue = [===["), "{text}");
    let back: Value = serde_json::from_str(extract(&text).expect("extract")).expect("parse");
    assert_eq!(back["probes"][0], entry);
}

#[test]
fn probe_ids_are_ulids_unique_and_monotonic() {
    let made: Vec<String> = (0..2000).map(|_| new_probe_id()).collect();
    assert!(made.iter().all(|id| is_probe_id(id)), "{made:?}");
    let mut sorted = made.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, made, "ids strictly increase");
    assert_eq!(encode(0, 0), "0".repeat(26));
    assert_eq!(
        encode(0xFFFF_FFFF_FFFF, u128::MAX),
        format!("7{}", "Z".repeat(25))
    );
    assert!(
        !is_probe_id("01JBX7Q2M3N4P5R6S7T8V9W0XI"),
        "I is not Crockford"
    );
}

#[test]
fn submit_writes_a_dev_queue_that_validates() {
    let wow = wow("ps-submit", &["_anniversary_"]);
    let s = store(&super::temp_dir("ps-submit-data"));
    let a = s
        .submit(&wow, &request("_anniversary_", 72), NOW)
        .expect("submit a");
    let b = s
        .submit(&wow, &request("_anniversary_", 24), NOW)
        .expect("submit b");
    assert_eq!(
        (a.status, a.flavor.as_str()),
        (ProbeStatus::Queued, "_anniversary_")
    );
    let q = queue_of(&wow, "_anniversary_");
    assert_eq!(schema_errors("probe-queue", &q), Vec::<String>::new());
    assert_eq!(
        ids(&q),
        vec![a.probe_id.clone(), b.probe_id.clone()],
        "oldest first"
    );
    assert_eq!(q["probes"][0]["flavor"], "_anniversary_");
    assert_eq!(a.request["flavor"], "_anniversary_");
    assert!(a.request.get("probeId").is_none());
    assert_eq!(
        schema_errors("probe-list", &list_of(&a)),
        Vec::<String>::new()
    );
}

#[test]
fn auto_picks_the_flavor_with_the_newest_ledgerlink_sv() {
    let wow = wow("ps-auto", &["_anniversary_", "_classic_era_", "_retail_"]);
    sv_at(&wow, "_anniversary_", 3600);
    sv_at(&wow, "_classic_era_", 10);
    let s = store(&super::temp_dir("ps-auto-data"));
    let rec = s.submit(&wow, &request("auto", 72), NOW).expect("submit");
    assert_eq!(rec.flavor, "_classic_era_");
    assert_eq!(rec.request["flavor"], "_classic_era_");
    assert_eq!(ids(&queue_of(&wow, "_classic_era_")), vec![rec.probe_id]);
}

#[test]
fn refused_submits_store_nothing() {
    let wow = wow("ps-refused", &["_anniversary_"]);
    std::fs::create_dir_all(wow.join("_retail_/Interface/AddOns")).expect("no LedgerLink");
    let data = super::temp_dir("ps-refused-data");
    let s = store(&data);
    let unknown = s.submit(&wow, &request("_ptr_", 72), NOW);
    assert!(matches!(
        unknown,
        Err(ProbeStoreError::Flavor(FlavorError::Unknown(_)))
    ));
    let auto = s.submit(&wow, &request("auto", 72), NOW);
    assert!(matches!(
        auto,
        Err(ProbeStoreError::Flavor(FlavorError::NoneSeen))
    ));
    let missing = s.submit(&wow, &request("_retail_", 72), NOW);
    assert!(
        matches!(
            missing,
            Err(ProbeStoreError::Write(WriteGuardError::NotInstalled(_)))
        ),
        "{missing:?}"
    );
    assert!(!wow.join("_retail_/Interface/AddOns/LedgerLink").exists());
    assert!(s.list(None).is_empty());
    assert!(store(&data).list(None).is_empty(), "nothing persisted");
}

#[test]
fn a_flavors_queue_holds_fifty_probes() {
    let wow = wow("ps-full", &["_anniversary_", "_retail_"]);
    let s = store(&super::temp_dir("ps-full-data"));
    for _ in 0..50 {
        s.submit(&wow, &request("_anniversary_", 72), NOW)
            .expect("fits");
    }
    let full = s.submit(&wow, &request("_anniversary_", 72), NOW);
    assert!(
        matches!(full, Err(ProbeStoreError::QueueFull(_))),
        "{full:?}"
    );
    assert_eq!(ids(&queue_of(&wow, "_anniversary_")).len(), 50);
    s.submit(&wow, &request("_retail_", 72), NOW)
        .expect("other flavor has room");
    assert_eq!(s.list(None).len(), 51);
}

#[test]
fn cancel_takes_the_probe_off_the_queue() {
    let wow = wow("ps-cancel", &["_anniversary_"]);
    let s = store(&super::temp_dir("ps-cancel-data"));
    let a = s
        .submit(&wow, &request("_anniversary_", 72), NOW)
        .expect("a");
    let b = s
        .submit(&wow, &request("_anniversary_", 72), NOW)
        .expect("b");
    let got = s.cancel(&wow, &a.probe_id, NOW).expect("cancel");
    assert_eq!(got.status, ProbeStatus::Cancelled);
    assert_eq!(ids(&queue_of(&wow, "_anniversary_")), vec![b.probe_id]);
    assert!(matches!(
        s.cancel(&wow, &a.probe_id, NOW),
        Err(ProbeStoreError::NotQueued(..))
    ));
    assert!(matches!(
        s.cancel(&wow, "nope", NOW),
        Err(ProbeStoreError::NotFound(_))
    ));
}

#[test]
fn probes_past_their_ttl_expire() {
    let wow = wow("ps-expire", &["_anniversary_"]);
    let s = store(&super::temp_dir("ps-expire-data"));
    let short = s
        .submit(&wow, &request("_anniversary_", 1), NOW)
        .expect("short");
    let long = s
        .submit(&wow, &request("_anniversary_", 72), NOW)
        .expect("long");
    assert!(s.expire_due(&wow, NOW + 3599).is_empty());
    assert_eq!(s.expire_due(&wow, NOW + 3600), vec![short.probe_id.clone()]);
    assert_eq!(
        s.get(&short.probe_id).map(|r| r.status),
        Some(ProbeStatus::Expired)
    );
    assert_eq!(ids(&queue_of(&wow, "_anniversary_")), vec![long.probe_id]);
    assert_eq!(s.list(Some(ProbeStatus::Expired)).len(), 1);
}

#[test]
fn answers_and_rejections_close_a_probe() {
    let wow = wow("ps-answer", &["_anniversary_"]);
    let s = store(&super::temp_dir("ps-answer-data"));
    let a = s
        .submit(&wow, &request("_anniversary_", 72), NOW)
        .expect("a");
    let b = s
        .submit(&wow, &request("_anniversary_", 72), NOW)
        .expect("b");
    let ans = s
        .apply_answer(&wow, &a.probe_id, Ok(json!({ "ok": 1 })), NOW + 60)
        .expect("ok");
    assert_eq!(
        (ans.status, ans.answered_at),
        (ProbeStatus::Answered, Some(NOW + 60))
    );
    assert_eq!(ans.result, Some(json!({ "ok": 1 })));
    let long = "x".repeat(600);
    let rej = s
        .apply_answer(&wow, &b.probe_id, Err(long), NOW + 61)
        .expect("rejected");
    assert_eq!(rej.status, ProbeStatus::Rejected);
    assert_eq!(rej.error.as_deref().map(str::len), Some(512));
    assert!(ids(&queue_of(&wow, "_anniversary_")).is_empty());
    let again = s.apply_answer(&wow, &a.probe_id, Ok(json!(null)), NOW + 62);
    assert!(matches!(again, Err(ProbeStoreError::NotQueued(..))));
    assert_eq!(
        schema_errors("probe-list", &list_of(&ans)),
        Vec::<String>::new()
    );
}

#[test]
fn the_store_survives_a_reload_and_a_corrupt_file_starts_empty() {
    let wow = wow("ps-reload", &["_anniversary_"]);
    let data = super::temp_dir("ps-reload-data");
    let a = store(&data)
        .submit(&wow, &request("_anniversary_", 72), NOW)
        .expect("a");
    let reloaded = store(&data);
    assert_eq!(reloaded.get(&a.probe_id), Some(a.clone()));
    assert_eq!(reloaded.list(Some(ProbeStatus::Queued)).len(), 1);
    let file = data.join("dev-bridge").join("probes.json");
    std::fs::write(&file, b"{not json").expect("corrupt");
    assert!(store(&data).list(None).is_empty());
}
