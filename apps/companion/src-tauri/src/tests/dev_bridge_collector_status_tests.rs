//! `/dev/v1/collectors` `status` + top-level `linked` (Raid-Ledger #1960):
//! the tile's own lines mapped to the closed enum, the device precedence,
//! and the served body against the pinned schema/fixtures.

use super::dev_bridge_unit_tests::schema_errors;
use crate::collector_status::{
    device_block, fallback, finished_status, row_status, DeviceBlock, LineStatus,
};
use crate::collector_view::device_state;
use crate::config::AppConfig;
use crate::dev_bridge::collectors::collectors_body;
use crate::manifest::{BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::{effect, stop_status, Runtime};
use crate::upload::{UploadOutcome, GALLERY_FULL_CODE};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

const WOW: &str = WOW_LEDGERLINK.id;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/dev-bridge/v1/fixtures")
}

fn load(rel: &str) -> Value {
    let text = std::fs::read_to_string(fixtures().join(rel)).expect(rel);
    serde_json::from_str(&text).expect(rel)
}

/// The schema's status enum: the string array that holds "up-to-date".
fn schema_statuses(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::Array(a) if a.iter().any(|x| x == "up-to-date") => Some(
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
        ),
        Value::Array(a) => a.iter().find_map(schema_statuses),
        Value::Object(o) => o.values().find_map(schema_statuses),
        _ => None,
    }
}

#[test]
fn every_status_is_exactly_the_pinned_enum() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../../contracts/dev-bridge/v1/schema.json"
    ))
    .expect("schema");
    let mut pinned = schema_statuses(&schema).expect("status enum in schema.json");
    let mut ours: Vec<String> = LineStatus::ALL
        .iter()
        .map(|s| s.as_str().to_owned())
        .collect();
    pinned.sort();
    ours.sort();
    assert_eq!(ours, pinned, "LineStatus drifted from dev-bridge/v1");
    let mut body = load("collectors.json");
    for s in LineStatus::ALL {
        body["collectors"][0]["status"] = Value::from(s.as_str());
        assert_eq!(
            schema_errors("collectors", &body),
            Vec::<String>::new(),
            "{s:?}"
        );
    }
}

#[test]
fn device_precedence_is_paused_then_rate_limited_then_link_then_error() {
    let unlinked = Ok(false);
    let st = |p, h, l: &Result<bool, String>| device_block(p, h, l).map(|b| b.status());
    assert_eq!(st(true, Some(9), &unlinked), Some(LineStatus::Paused));
    assert_eq!(st(false, Some(9), &unlinked), Some(LineStatus::RateLimited));
    assert_eq!(st(false, None, &unlinked), Some(LineStatus::LinkDevice));
    assert_eq!(
        st(false, None, &Err("locked".into())),
        Some(LineStatus::Error)
    );
    assert_eq!(st(false, None, &Ok(true)), None);
    assert_eq!(
        device_block(false, Some(9), &Ok(true)),
        Some(DeviceBlock::RateLimited(9))
    );
    // The window's device line is rendered from the same block.
    for (p, h, l) in [
        (true, None, Ok(true)),
        (false, Some(3), Ok(true)),
        (false, None, Ok(false)),
        (false, None, Err("x".to_owned())),
        (false, None, Ok(true)),
    ] {
        assert_eq!(
            device_state(p, h, l.clone()).is_some(),
            device_block(p, h, &l).is_some(),
            "{p} {h:?} {l:?}"
        );
    }
}

#[test]
fn a_row_is_off_when_switched_off_else_the_device_block_wins() {
    let dev = Some(LineStatus::LinkDevice);
    let line = Some(LineStatus::Uploaded);
    assert_eq!(row_status(false, dev, line), LineStatus::Off);
    assert_eq!(row_status(true, dev, line), LineStatus::LinkDevice);
    assert_eq!(row_status(true, None, line), LineStatus::Uploaded);
    assert_eq!(row_status(true, None, None), LineStatus::NotScanned);
    assert_eq!(fallback(true).text, "Waiting for the first scan…");
    assert_eq!(fallback(false).status, LineStatus::Off);
}

#[test]
fn a_finished_pass_is_uploaded_only_when_one_was_accepted() {
    assert_eq!(finished_status(1, 0), LineStatus::Uploaded);
    assert_eq!(finished_status(3, 2), LineStatus::Uploaded);
    assert_eq!(
        finished_status(0, 2),
        LineStatus::Waiting,
        "given-up items retry"
    );
    assert_eq!(
        finished_status(0, 0),
        LineStatus::UpToDate,
        "all refused + settled"
    );
}

#[test]
fn a_stopped_batch_maps_by_what_stopped_it() {
    let full = UploadOutcome::Rejected {
        status: 413,
        code: GALLERY_FULL_CODE.to_owned(),
        issue: None,
    };
    let cases = [
        (UploadOutcome::Unauthorized, LineStatus::LinkDevice),
        (
            UploadOutcome::RateLimited {
                wait: Duration::from_secs(30),
            },
            LineStatus::RateLimited,
        ),
        (full, LineStatus::Rejected),
        (
            UploadOutcome::Deferred {
                reason: "not yet".into(),
            },
            LineStatus::Retrying,
        ),
    ];
    for (outcome, want) in cases {
        let fx = effect(&outcome);
        assert!(fx.stop, "{outcome:?} stops the batch");
        assert_eq!(stop_status(&fx), want, "{outcome:?}");
    }
}

fn served(rt: &Runtime, cfg: &AppConfig) -> Value {
    let body = collectors_body(&rt.snapshot(cfg), false);
    assert_eq!(
        schema_errors("collectors", &body),
        Vec::<String>::new(),
        "{body}"
    );
    body
}

fn status_of(body: &Value, id: &str) -> String {
    let rows = body["collectors"].as_array().expect("rows");
    let row = rows.iter().find(|r| r["id"] == id).expect("row");
    row["status"].as_str().expect("status").to_owned()
}

#[test]
fn the_served_body_carries_each_rows_status_and_device_linked() {
    let rt = Runtime::new(super::temp_dir("bridge-status").join("sent.json"));
    let mut cfg = AppConfig::default();
    cfg.set_collector_enabled(WOW, true);
    cfg.set_collector_enabled(BG3_SAVE.id, false);
    let body = served(&rt, &cfg);
    assert_eq!(body["linked"], Value::Bool(true));
    assert_eq!(status_of(&body, WOW), "not-scanned");
    assert_eq!(status_of(&body, BG3_SAVE.id), "off");

    rt.set_statuses(BTreeMap::from([(WOW.to_owned(), LineStatus::Uploaded)]));
    assert_eq!(status_of(&served(&rt, &cfg), WOW), "uploaded");

    rt.set_device(false, Ok(false));
    let body = served(&rt, &cfg);
    assert_eq!(body["linked"], Value::Bool(false));
    assert_eq!(status_of(&body, WOW), "link-device");
    assert_eq!(status_of(&body, BG3_SAVE.id), "off");

    rt.set_device(true, Ok(false));
    assert_eq!(status_of(&served(&rt, &cfg), WOW), "paused");

    rt.set_device(false, Err("keychain locked".into()));
    let body = served(&rt, &cfg);
    assert_eq!(body.get("linked"), None, "unknown is omitted, not guessed");
    assert_eq!(status_of(&body, WOW), "error");
}

#[test]
fn the_pinned_collectors_fixtures_agree_with_the_schema() {
    for f in [
        "collectors.json",
        "collectors--unlinked.json",
        "collectors--without-status.json",
    ] {
        assert_eq!(
            schema_errors("collectors", &load(f)),
            Vec::<String>::new(),
            "{f}"
        );
    }
    for f in [
        "collectors--unknown-status",
        "collectors--linked-not-boolean",
        "collectors--row-linked",
    ] {
        let errors = schema_errors("collectors", &load(&format!("invalid/{f}.json")));
        assert!(!errors.is_empty(), "{f} must be rejected");
    }
}
