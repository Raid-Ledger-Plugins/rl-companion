//! `/dev/v1/collectors` reports what the runtime actually did (lastScanAt,
//! pending, a restart-proof lastUploadAt), and Activity lines reach the
//! `/dev/v1/logs` ring without names or paths.

use super::dev_bridge_unit_tests::schema_errors;
use crate::collector::{PendingUpload, SentState};
use crate::config::AppConfig;
use crate::dev_bridge::collectors::collectors_body;
use crate::dev_bridge::logbuf;
use crate::manifest::{BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::{effect, Runtime};
use crate::scan_report::{bridge_line, upload_times_path, UPLOAD_TIMES_FILE};
use crate::upload::UploadOutcome;
use serde_json::{json, Value};
use std::path::Path;

const WOW: &str = WOW_LEDGERLINK.id;

fn item(contract: &'static str, item: &str, sha: &str) -> PendingUpload {
    PendingUpload {
        campaign: None,
        contract,
        scope: "https://rl.test|/acct/LedgerLink.lua".into(),
        item: item.into(),
        sha256: sha.into(),
        label: "Thrall".into(),
        body: json!({}),
        blobs: vec![],
    }
}

fn wow_on() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.set_collector_enabled(WOW, true);
    cfg
}

/// The contract-validated `/dev/v1/collectors` row for `id`.
fn row(rt: &Runtime, cfg: &AppConfig, id: &str) -> Value {
    let body = collectors_body(&rt.snapshot(cfg), false);
    let errors = schema_errors("collectors", &body);
    assert!(
        errors.is_empty(),
        "body fails the pinned schema: {errors:?}"
    );
    let rows = body["collectors"].as_array().expect("collectors array");
    rows.iter()
        .find(|r| r["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no row for {id} in {body}"))
}

fn accepted() -> crate::runtime::Effect {
    effect(&UploadOutcome::Accepted)
}

#[test]
fn a_scan_stamps_last_scan_at_and_counts_what_it_left_unsettled() {
    let rt = Runtime::new(super::temp_dir("scan-report-scan").join("sent.json"));
    let before = row(&rt, &wow_on(), WOW);
    assert_eq!(before["lastScanAt"], Value::Null, "no scan yet: {before}");
    rt.scanned(WOW, &[item(WOW, "g1", "aa"), item(WOW, "g2", "bb")]);
    let after = row(&rt, &wow_on(), WOW);
    assert!(after["lastScanAt"].is_string(), "scan not stamped: {after}");
    assert_eq!(after["pending"], 2, "two unsent exports: {after}");
}

#[test]
fn a_nothing_new_scan_still_stamps_with_zero_pending() {
    let rt = Runtime::new(super::temp_dir("scan-report-empty").join("sent.json"));
    rt.scanned(WOW, &[]);
    let r = row(&rt, &wow_on(), WOW);
    assert!(
        r["lastScanAt"].is_string(),
        "up-to-date scan not stamped: {r}"
    );
    assert_eq!(r["pending"], 0, "{r}");
}

#[test]
fn pending_drops_what_the_server_settled() {
    let rt = Runtime::new(super::temp_dir("scan-report-settled").join("sent.json"));
    let (sent, left) = (item(WOW, "g1", "aa"), item(WOW, "g2", "bb"));
    rt.finish(&sent, &accepted());
    rt.scanned(WOW, &[sent, left]);
    let r = row(&rt, &wow_on(), WOW);
    assert_eq!(r["pending"], 1, "only g2 is still queued: {r}");
    assert!(
        r["lastUploadAt"].is_string(),
        "accepted upload missing: {r}"
    );
}

#[test]
fn last_upload_at_survives_a_restart() {
    let sent_path = super::temp_dir("scan-report-restart").join("sent.json");
    let first = Runtime::new(sent_path.clone());
    first.finish(&item(WOW, "g1", "aa"), &accepted());
    let stamped = row(&first, &wow_on(), WOW)["lastUploadAt"].clone();
    assert!(stamped.is_string(), "no upload time before restart");
    let after_restart = Runtime::new(sent_path);
    let r = row(&after_restart, &wow_on(), WOW);
    assert_eq!(
        r["lastUploadAt"], stamped,
        "restart lost the upload time: {r}"
    );
    assert_eq!(
        r["lastScanAt"],
        Value::Null,
        "a fresh process has not scanned"
    );
}

#[test]
fn a_refused_upload_sets_no_upload_time() {
    let rt = Runtime::new(super::temp_dir("scan-report-refused").join("sent.json"));
    let refused = effect(&UploadOutcome::Deferred {
        reason: "server busy".into(),
    });
    rt.finish(&item(WOW, "g1", "aa"), &refused);
    let r = row(&rt, &wow_on(), WOW);
    assert_eq!(
        r["lastUploadAt"],
        Value::Null,
        "deferred is not accepted: {r}"
    );
}

#[test]
fn an_old_sent_state_without_upload_times_still_loads() {
    let dir = super::temp_dir("scan-report-old-file");
    let sent_path = dir.join("sent.json");
    let old = json!({ "https://rl.test|/acct/LedgerLink.lua": { "g1": "aa" } });
    std::fs::write(&sent_path, old.to_string()).expect("write old sent-state");
    assert!(!upload_times_path(&sent_path).exists());
    let rt = Runtime::new(sent_path.clone());
    let r = row(&rt, &wow_on(), WOW);
    assert_eq!(r["lastUploadAt"], Value::Null, "{r}");
    rt.scanned(WOW, &[item(WOW, "g1", "aa")]);
    assert_eq!(row(&rt, &wow_on(), WOW)["pending"], 0, "dedupe kept");
    assert_eq!(
        rt.sent(),
        SentState::load(&sent_path),
        "sent-state untouched"
    );
}

#[test]
fn upload_times_live_beside_sent_state() {
    let p = upload_times_path(Path::new("/data/app/sent-state.json"));
    assert_eq!(p, Path::new("/data/app").join(UPLOAD_TIMES_FILE));
}

#[test]
fn a_disabled_collector_reports_no_scan_and_nothing_pending() {
    let rt = Runtime::new(super::temp_dir("scan-report-off").join("sent.json"));
    // bg3-save is opt-in (off by default); a scan recorded before the switch
    // flipped must not show through.
    rt.scanned(BG3_SAVE.id, &[item(BG3_SAVE.id, "save", "aa")]);
    let r = row(&rt, &AppConfig::default(), BG3_SAVE.id);
    assert_eq!(r["enabled"], false, "{r}");
    assert_eq!(
        r["lastScanAt"],
        Value::Null,
        "off collector never scanned: {r}"
    );
    assert_eq!(r["pending"], 0, "{r}");
    rt.forget_scan(WOW);
    assert_eq!(row(&rt, &wow_on(), WOW)["lastScanAt"], Value::Null);
}

fn ring_line(marker: &str) -> Value {
    logbuf::global()
        .select(0, None, None)
        .into_iter()
        .map(|(_, v)| v)
        .find(|v| v["msg"].as_str().is_some_and(|m| m.contains(marker)))
        .unwrap_or_else(|| panic!("no ring line carries {marker}"))
}

#[test]
fn activity_lines_reach_the_bridge_ring_without_the_name() {
    let rt = Runtime::new(super::temp_dir("scan-report-log").join("sent.json"));
    rt.log_for(WOW, "Thrall: Rejected 422: marker_q81");
    let line = ring_line("marker_q81");
    assert_eq!(line["msg"], "Rejected 422: marker_q81", "{line}");
    assert_eq!(line["target"], "collector:wow-ledgerlink", "{line}");
    assert_eq!(line["level"], "info");
    let window = &rt.snapshot(&AppConfig::default()).log;
    assert!(
        window.iter().any(|l| l.text.contains("Thrall")),
        "window keeps it"
    );
}

#[test]
fn device_activity_lines_reach_the_ring_with_paths_scrubbed() {
    let rt = Runtime::new(super::temp_dir("scan-report-log-path").join("sent.json"));
    rt.log(r"Cannot watch D:\Games\World of Warcraft\_retail_ (marker_p42)");
    let line = ring_line("marker_p42");
    assert_eq!(line["target"], "collector", "{line}");
    let msg = line["msg"].as_str().unwrap_or_default();
    assert!(!msg.contains("Games"), "path leaked: {msg}");
    assert!(msg.starts_with("Cannot watch <path>"), "{msg}");
}

#[test]
fn bridge_line_drops_the_subject_and_scrubs_paths() {
    assert_eq!(bridge_line(Some(WOW), "Thrall: Uploaded"), "Uploaded");
    assert_eq!(
        bridge_line(Some(WOW), "Acct: cannot read /mnt/wow/WTF/x.lua (denied)"),
        "cannot read <path> (denied)"
    );
    assert_eq!(
        bridge_line(None, "Could not save (disk full)"),
        "Could not save (disk full)"
    );
    assert_eq!(
        bridge_line(Some(WOW), "LedgerLink uploads turned on"),
        "LedgerLink uploads turned on"
    );
}
