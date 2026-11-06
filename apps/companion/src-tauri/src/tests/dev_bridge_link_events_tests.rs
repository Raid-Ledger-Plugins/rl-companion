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
    rt.end_scan(rt.begin_scan(), false, Ok(true), uploaded_lines(), false);
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
    rt.end_scan(rt.begin_scan(), false, Ok(true), uploaded_lines(), false);
    rt.end_scan(rt.begin_scan(), false, Ok(false), tokenless_lines(), false);
    assert_eq!(recorded(log), one, "first scan without a token");
    rt.end_scan(rt.begin_scan(), false, Ok(false), tokenless_lines(), false);
    assert_eq!(recorded(log), one, "a second tokenless scan recorded again");
    // Linked again, then unlinked again: a new transition, a new event.
    rt.end_scan(rt.begin_scan(), false, Ok(true), uploaded_lines(), false);
    rt.unlinked();
    rt.end_scan(rt.begin_scan(), false, Ok(false), tokenless_lines(), false);
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
    rt.end_scan(rt.begin_scan(), false, Ok(true), relink, true);
    // … so the next scan, which finds the token gone, adds none.
    rt.end_scan(rt.begin_scan(), false, Ok(false), tokenless_lines(), false);
    assert_eq!(recorded(log).len(), 1, "the 401's event was doubled");
}

/// One full scan, begun and ended back to back.
fn scan(rt: &Runtime, linked: Result<bool, String>, lines: BTreeMap<String, Line>) {
    rt.end_scan(rt.begin_scan(), false, linked, lines, false);
}

fn relink_lines() -> BTreeMap<String, Line> {
    BTreeMap::from([(
        WOW.to_owned(),
        Line::new(LineStatus::LinkDevice, "Re-link this device"),
    )])
}

#[test]
fn a_scan_running_across_unlink_does_not_write_linked_back() {
    let (rt, log) = runtime("bridge-unlink-mid-scan");
    let cfg = both_on();
    scan(&rt, Ok(true), uploaded_lines());
    // The scan read the token, then the user clicked Unlink before it ended.
    let gen = rt.begin_scan();
    rt.unlinked();
    rt.end_scan(gen, false, Ok(true), uploaded_lines(), false);
    let body = collectors_body(&rt.snapshot(&cfg), false);
    assert_eq!(
        body["linked"],
        Value::Bool(false),
        "a scan running across Unlink wrote linked:true back"
    );
    assert_eq!(status_of(&body, WOW), "link-device", "WoW row after Unlink");
    // The scan Unlink kicked records the transition's one event.
    scan(&rt, Ok(false), tokenless_lines());
    assert_eq!(recorded(log).len(), 1, "the Unlink transition's events");
}

#[test]
fn a_re_link_during_a_tokenless_scan_records_no_second_link_required() {
    let (rt, log) = runtime("bridge-relink-mid-scan");
    scan(&rt, Ok(true), uploaded_lines());
    scan(&rt, Ok(false), tokenless_lines());
    assert_eq!(recorded(log).len(), 1, "the unlinked spell's event");
    // A tokenless scan is running when the user links the device again.
    let gen = rt.begin_scan();
    rt.linked();
    rt.end_scan(gen, false, Ok(false), tokenless_lines(), false);
    assert_eq!(
        recorded(log).len(),
        1,
        "a re-link during a tokenless scan recorded a second link-required"
    );
    // The scan the link kicked sees the token: linked, and re-armed.
    scan(&rt, Ok(true), uploaded_lines());
    let body = collectors_body(&rt.snapshot(&both_on()), false);
    assert_eq!(body["linked"], Value::Bool(true), "after the re-link scan");
}

#[test]
fn a_401_in_a_scan_running_across_unlink_is_still_one_link_required() {
    let (rt, log) = runtime("bridge-401-across-unlink");
    let wow = CollectorId::known(WOW).expect("manifest id");
    scan(&rt, Ok(true), uploaded_lines());
    let gen = rt.begin_scan();
    // The pass's upload 401s (recording its own event), and the user unlinks.
    log.push_now(for_outcome(wow, &UploadOutcome::Unauthorized, 0), 1);
    rt.unlinked();
    rt.end_scan(gen, false, Ok(true), relink_lines(), true);
    assert!(
        !rt.snapshot(&both_on()).relink,
        "a stale 401 raised the re-link notice after the user unlinked"
    );
    scan(&rt, Ok(false), tokenless_lines());
    assert_eq!(recorded(log).len(), 1, "the 401's event was doubled");
}

#[test]
fn a_link_re_arms_link_required() {
    let (rt, log) = runtime("bridge-rearm-link");
    scan(&rt, Ok(true), uploaded_lines());
    scan(&rt, Ok(false), tokenless_lines());
    rt.linked();
    // The link's token is refused or lost before any scan saw it linked.
    scan(&rt, Ok(false), tokenless_lines());
    assert_eq!(recorded(log).len(), 2, "linked() did not re-arm the event");
}

#[test]
fn a_server_switch_to_a_linked_server_re_arms_link_required() {
    let (rt, log) = runtime("bridge-rearm-server");
    scan(&rt, Ok(true), uploaded_lines());
    scan(&rt, Ok(false), tokenless_lines());
    rt.server_changed(true);
    let body = collectors_body(&rt.snapshot(&both_on()), false);
    assert_eq!(body["linked"], Value::Bool(true), "after the switch");
    scan(&rt, Ok(false), tokenless_lines());
    assert_eq!(
        recorded(log).len(),
        2,
        "server_changed(true) did not re-arm the event"
    );
}

#[test]
fn a_config_401_then_tokenless_scans_record_one_link_required() {
    let (rt, log) = runtime("bridge-config-401");
    scan(&rt, Ok(true), uploaded_lines());
    rt.config_unauthorized();
    let body = collectors_body(&rt.snapshot(&both_on()), false);
    assert_eq!(body["linked"], Value::Bool(false), "after the /config 401");
    assert_eq!(recorded(log), vec![], "the /config 401 records no event");
    scan(&rt, Ok(false), tokenless_lines());
    scan(&rt, Ok(false), tokenless_lines());
    let one = vec![("\"link-required\"".to_owned(), format!("\"{WOW}\""))];
    assert_eq!(recorded(log), one, "a /config 401's spell");
}

/// A `Shared` on `origin` with an in-memory token store.
fn shared_on(name: &str, origin: &str) -> crate::state::Shared {
    let shared = crate::state::Shared::new(
        super::temp_dir(name).join("config.json"),
        Box::new(crate::token::MemoryStore::default()),
    );
    shared
        .update_config(|c| c.server_url = origin.to_owned())
        .expect("save config");
    shared
}

#[test]
fn re_link_mid_scan_then_the_old_tokens_401_leaves_the_new_token_in_place() {
    use crate::token::DeviceToken;
    let origin = "https://raid.example";
    let shared = shared_on("upload-401-relink", origin);
    let (rt, _) = runtime("upload-401-relink-rt");
    let old = DeviceToken::parse(super::TOKEN).expect("token");
    let new = DeviceToken::parse(&format!("rlcd_{}", "N".repeat(43))).expect("token");
    shared.save_token(origin, &old).expect("save");
    // The scan read `old` and sent it; the user re-linked before its 401.
    let gen = rt.begin_scan();
    shared.save_token(origin, &new).expect("re-link");
    rt.linked();
    let (line, stop) = crate::runtime::upload_refused(&shared, &rt, origin, &old);
    rt.end_scan(
        gen,
        false,
        Ok(true),
        BTreeMap::from([(WOW.to_owned(), line)]),
        stop,
    );
    assert_eq!(
        shared.token().expect("load").map(|t| t.hint().to_owned()),
        Some(new.hint().to_owned()),
        "the old token's late 401 deleted the re-linked one"
    );
    assert!(
        !rt.snapshot(&both_on()).relink,
        "a stale 401 asked for a re-link after a good link"
    );
    // A 401 for the token still stored does forget it.
    let _ = crate::runtime::upload_refused(&shared, &rt, origin, &new);
    assert!(
        shared.token().expect("load").is_none(),
        "the refused token was kept"
    );
}

#[test]
fn a_config_401_during_a_scan_keeps_the_re_link_notice_at_its_end() {
    let (rt, _) = runtime("config-401-mid-scan");
    scan(&rt, Ok(true), uploaded_lines());
    let gen = rt.begin_scan();
    // `/config` answers 401 (token cleared) while the upload scan runs.
    rt.config_unauthorized();
    rt.end_scan(gen, false, Ok(true), uploaded_lines(), false);
    assert!(
        rt.snapshot(&both_on()).relink,
        "a scan ending after a /config 401 cleared its re-link notice"
    );
}

#[test]
fn a_switch_away_and_back_mid_scan_then_a_401_still_revokes_the_current_token() {
    use crate::token::DeviceToken;
    let (a, b) = ("https://raid.example", "https://other.example");
    let shared = shared_on("upload-401-switch-back", a);
    let (rt, log) = runtime("upload-401-switch-back-rt");
    let token = DeviceToken::parse(super::TOKEN).expect("token");
    shared.save_token(a, &token).expect("save");
    scan(&rt, Ok(true), uploaded_lines());
    // The scan sent A's token; the user switches A→B→A before its 401.
    let gen = rt.begin_scan();
    shared
        .update_config(|c| c.server_url = b.to_owned())
        .expect("to B");
    rt.server_changed(false);
    shared
        .update_config(|c| c.server_url = a.to_owned())
        .expect("to A");
    rt.server_changed(true);
    let wow = CollectorId::known(WOW).expect("manifest id");
    log.push_now(for_outcome(wow, &UploadOutcome::Unauthorized, 0), 1);
    let (line, stop) = crate::runtime::upload_refused(&shared, &rt, a, &token);
    let lines = BTreeMap::from([(WOW.to_owned(), line)]);
    rt.end_scan(gen, false, Ok(true), lines, stop);
    assert!(
        shared.token().expect("load").is_none(),
        "A's still-current token survived its 401"
    );
    let snap = rt.snapshot(&both_on());
    assert!(snap.relink, "the revoking 401 raised no re-link notice");
    let body = collectors_body(&snap, false);
    assert_eq!(body["linked"], Value::Bool(false), "no token, still linked");
    scan(&rt, Ok(false), tokenless_lines());
    assert_eq!(recorded(log).len(), 1, "the 401's event was doubled");
}

#[test]
fn a_stale_401_after_a_switch_to_linked_b_then_unlink_b_records_b_s_link_required() {
    use crate::token::DeviceToken;
    let (a, b) = ("https://raid.example", "https://other.example");
    let shared = shared_on("upload-401-stale-switch", a);
    let (rt, log) = runtime("upload-401-stale-switch-rt");
    let token = DeviceToken::parse(super::TOKEN).expect("token");
    shared.save_token(a, &token).expect("save A");
    shared.save_token(b, &token).expect("save B");
    scan(&rt, Ok(true), uploaded_lines());
    // The scan sent A's token; the user switches to the linked B before its 401.
    let gen = rt.begin_scan();
    shared
        .update_config(|c| c.server_url = b.to_owned())
        .expect("to B");
    rt.server_changed(true);
    let wow = CollectorId::known(WOW).expect("manifest id");
    log.push_now(for_outcome(wow, &UploadOutcome::Unauthorized, 0), 1);
    let (line, stop) = crate::runtime::upload_refused(&shared, &rt, a, &token);
    let lines = BTreeMap::from([(WOW.to_owned(), line)]);
    rt.end_scan(gen, false, Ok(true), lines, stop);
    assert_eq!(recorded(log).len(), 1, "A's 401 records its own event");
    // B is unlinked before any linked scan: B's own spell starts now.
    rt.unlinked();
    scan(&rt, Ok(false), tokenless_lines());
    scan(&rt, Ok(false), tokenless_lines());
    assert_eq!(
        recorded(log).len(),
        2,
        "A's stale 401 suppressed (or doubled) B's link-required"
    );
}

#[test]
fn a_link_after_the_401_cleared_the_token_keeps_the_new_link_at_scan_end() {
    use crate::token::DeviceToken;
    let origin = "https://raid.example";
    let shared = shared_on("upload-401-then-link", origin);
    let (rt, _) = runtime("upload-401-then-link-rt");
    let old = DeviceToken::parse(super::TOKEN).expect("token");
    let new = DeviceToken::parse(&format!("rlcd_{}", "N".repeat(43))).expect("token");
    shared.save_token(origin, &old).expect("save");
    scan(&rt, Ok(true), uploaded_lines());
    let gen = rt.begin_scan();
    // The 401 clears `old`; a Link completes before the scan ends.
    let (line, stop) = crate::runtime::upload_refused(&shared, &rt, origin, &old);
    shared.save_token(origin, &new).expect("link");
    rt.linked();
    let lines = BTreeMap::from([(WOW.to_owned(), line)]);
    rt.end_scan(gen, false, Ok(true), lines, stop);
    let snap = rt.snapshot(&both_on());
    assert!(
        !snap.relink,
        "the scan's 401 asked for a re-link after the Link"
    );
    let body = collectors_body(&snap, false);
    assert_eq!(
        body["linked"],
        Value::Bool(true),
        "the new link was overwritten"
    );
}

#[test]
fn a_switch_mid_scan_then_a_401_that_cleared_nothing_stays_linked() {
    use crate::token::DeviceToken;
    let (a, b) = ("https://raid.example", "https://other.example");
    let shared = shared_on("upload-401-switch-cleared-nothing", a);
    let (rt, _) = runtime("upload-401-switch-cleared-nothing-rt");
    let token = DeviceToken::parse(super::TOKEN).expect("token");
    shared.save_token(a, &token).expect("save A");
    shared.save_token(b, &token).expect("save B");
    scan(&rt, Ok(true), uploaded_lines());
    // The scan sent A's token; the user switches to the linked B before its 401.
    let gen = rt.begin_scan();
    shared
        .update_config(|c| c.server_url = b.to_owned())
        .expect("to B");
    rt.server_changed(true);
    let (line, stop) = crate::runtime::upload_refused(&shared, &rt, a, &token);
    assert_eq!(
        stop,
        crate::runtime::Relink::Refused,
        "A's 401 cleared something"
    );
    let lines = BTreeMap::from([(WOW.to_owned(), line)]);
    rt.end_scan(gen, false, Ok(true), lines, stop);
    assert!(
        shared.token().expect("load").is_some(),
        "B's token was cleared"
    );
    let snap = rt.snapshot(&both_on());
    assert!(
        !snap.relink,
        "a stale 401 that cleared nothing raised a re-link"
    );
    let body = collectors_body(&snap, false);
    assert_eq!(
        body["linked"],
        Value::Bool(true),
        "a stale 401 that cleared nothing unlinked B"
    );
}
