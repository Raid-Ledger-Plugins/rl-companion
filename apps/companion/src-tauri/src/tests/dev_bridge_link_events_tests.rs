//! The device's link state on the bridge: an Unlink shows at once (not on
//! the next scan), and each linked→unlinked transition records exactly one
//! `link-required` event — a 401's own, or one for an absent token.

use super::dev_bridge_unit_tests::schema_errors;
use crate::collector_status::{Line, LineStatus};
use crate::config::AppConfig;
use crate::dev_bridge::collectors::collectors_body;
use crate::dev_bridge::events::{for_outcome, CollectorId, EventLog};
use crate::manifest::{BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::Runtime;
use crate::upload::UploadOutcome;
use serde_json::Value;
use std::collections::BTreeMap;

const WOW: &str = WOW_LEDGERLINK.id;

fn both_on() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.set_collector_enabled(WOW, true);
    cfg.set_collector_enabled(BG3_SAVE.id, true);
    cfg
}

fn runtime(name: &str) -> (Runtime, &'static EventLog) {
    let log: &'static EventLog = Box::leak(Box::default());
    let rt = Runtime::with_events(super::temp_dir(name).join("sent.json"), log);
    (rt, log)
}

fn status_of(body: &Value, id: &str) -> String {
    let rows = body["collectors"].as_array().expect("rows");
    let row = rows.iter().find(|r| r["id"] == id).expect("row");
    row["status"].as_str().expect("status").to_owned()
}

/// The lines of a scan with no token: WoW has exports waiting, BG3 none.
fn tokenless_lines() -> BTreeMap<String, Line> {
    BTreeMap::from([
        (
            BG3_SAVE.id.to_owned(),
            Line::new(LineStatus::UpToDate, "Up to date"),
        ),
        (
            WOW.to_owned(),
            Line::new(LineStatus::Waiting, "2 export(s) waiting"),
        ),
    ])
}

fn uploaded_lines() -> BTreeMap<String, Line> {
    let up = || Line::new(LineStatus::Uploaded, "Uploaded 1 of 1 export(s)");
    BTreeMap::from([(WOW.to_owned(), up()), (BG3_SAVE.id.to_owned(), up())])
}

/// `(kind, collectorId)` of every recorded event, oldest first.
fn recorded(log: &EventLog) -> Vec<(String, String)> {
    let page = log.page(None, None).expect("events page");
    let rows = page["events"].as_array().expect("events").clone();
    rows.iter()
        .map(|e| (e["kind"].to_string(), e["collectorId"].to_string()))
        .collect()
}

#[test]
fn unlink_shows_link_device_at_once_not_on_the_next_scan() {
    let (rt, _) = runtime("bridge-unlink");
    let cfg = both_on();
    rt.end_scan(false, Ok(true), uploaded_lines(), false);
    rt.unlinked();
    let body = collectors_body(&rt.snapshot(&cfg), false);
    assert_eq!(schema_errors("collectors", &body), Vec::<String>::new());
    assert_eq!(
        body["linked"],
        Value::Bool(false),
        "after Unlink the bridge still said linked"
    );
    assert_eq!(status_of(&body, WOW), "link-device", "WoW row after Unlink");
    assert_eq!(status_of(&body, BG3_SAVE.id), "link-device", "BG3 row");
}

#[test]
fn an_absent_token_records_one_link_required_per_transition() {
    let (rt, log) = runtime("bridge-absent-token");
    let one = vec![("\"link-required\"".to_owned(), format!("\"{WOW}\""))];
    rt.end_scan(false, Ok(true), uploaded_lines(), false);
    rt.end_scan(false, Ok(false), tokenless_lines(), false);
    assert_eq!(recorded(log), one, "first scan without a token");
    rt.end_scan(false, Ok(false), tokenless_lines(), false);
    assert_eq!(recorded(log), one, "a second tokenless scan recorded again");
    // Linked again, then unlinked again: a new transition, a new event.
    rt.end_scan(false, Ok(true), uploaded_lines(), false);
    rt.unlinked();
    rt.end_scan(false, Ok(false), tokenless_lines(), false);
    assert_eq!(recorded(log).len(), 2, "the next transition recorded none");
}

#[test]
fn a_401_then_a_tokenless_scan_is_still_one_link_required() {
    let (rt, log) = runtime("bridge-401-then-absent");
    let wow = CollectorId::known(WOW).expect("manifest id");
    // The 401 records its own event as the upload's outcome …
    log.push_now(for_outcome(wow, &UploadOutcome::Unauthorized, 0), 1);
    let relink = BTreeMap::from([(
        WOW.to_owned(),
        Line::new(LineStatus::LinkDevice, "Re-link this device"),
    )]);
    rt.end_scan(false, Ok(true), relink, true);
    // … so the next scan, which finds the token gone, adds none.
    rt.end_scan(false, Ok(false), tokenless_lines(), false);
    assert_eq!(recorded(log).len(), 1, "the 401's event was doubled");
}
