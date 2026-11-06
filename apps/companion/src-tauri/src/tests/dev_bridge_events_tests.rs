//! `GET /dev/v1/events`: structured rows only, validated against the pinned
//! `dev-bridge/v1` `events` def; since/cursor paging; route-scoped cursors;
//! server codes map through the enum; the ring evicts at capacity.

use super::dev_bridge_unit_tests::schema_errors;
use crate::dev_bridge::auth::AuthPolicy;
use crate::dev_bridge::events::{
    encode, for_outcome, CollectorId, ErrorCode, Event, EventLog, Kind, CAPACITY,
};
use crate::dev_bridge::paging::{encode_cursor, PAGE_MAX_BYTES};
use crate::dev_bridge::server::{ServerCtx, Snapshot};
use crate::dev_bridge::token::sha256_hex;
use crate::upload::{classify, Attempt, UploadOutcome};
use hyper::Method;
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

const CALLER: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 229);
const FIX: &str = "../../../contracts/dev-bridge/v1/fixtures";

fn token() -> String {
    format!("rlcdev_{}", "A".repeat(43))
}

fn ctx(log: &'static EventLog) -> ServerCtx {
    let ctx = ServerCtx::with_events(log);
    ctx.set_snapshot(Snapshot {
        policy: AuthPolicy {
            allowlist: vec![CALLER],
            token_sha256: Some(sha256_hex(&token())),
        },
        wow_dir: String::new(),
        probes_enabled: false,
        snapshot_cache: None,
    });
    ctx
}

fn get(ctx: &ServerCtx, path: &str, query: Option<&str>) -> (u16, Value) {
    let auth = format!("Bearer {}", token());
    ctx.route(IpAddr::V4(CALLER), &Method::GET, path, query, Some(&auth))
}

fn id(s: &str) -> CollectorId {
    CollectorId::known(s).unwrap_or_else(|| panic!("{s} is a built-in collector"))
}

fn ts(n: usize) -> String {
    format!("2026-11-01T18:{:02}:{:02}Z", (n / 60) % 60, n % 60)
}

fn leaked() -> &'static EventLog {
    Box::leak(Box::new(EventLog::default()))
}

/// A widest-field row, so paging is forced quickly.
fn wide(c: CollectorId) -> Event {
    Event::new(c, Kind::UploadRejected)
        .count(1_000_000)
        .status(599)
        .code(ErrorCode::RetiredContractVersion)
}

fn fixture(name: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIX)
        .join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).expect("fixture parses")
}

#[test]
fn pinned_events_fixtures_behave_as_the_schema_says() {
    for ok in ["events.json", "events--empty.json", "events--max-page.json"] {
        assert_eq!(
            schema_errors("events", &fixture(ok)),
            Vec::<String>::new(),
            "{ok}"
        );
    }
    for bad in [
        "events--error-code-sentence.json",
        "events--free-text-message.json",
        "events--http-status-out-of-range.json",
        "events--missing-next.json",
        "events--unknown-error-code.json",
        "events--unknown-kind.json",
    ] {
        let errs = schema_errors("events", &fixture(&format!("invalid/{bad}")));
        assert!(!errs.is_empty(), "invalid/{bad} must fail the events def");
    }
    // Over-size is a page-cap rule (16 KiB), not a schema one.
    let over = fixture("invalid/events--over-size.json");
    let len = serde_json::to_vec(&over).expect("json").len();
    assert!(len > PAGE_MAX_BYTES, "over-size fixture is {len} B");
}

#[test]
fn served_pages_validate_fit_and_reassemble_in_order() {
    let log = leaked();
    let c = id("wow-ledgerlink");
    for n in 0..200 {
        log.push_at(wide(c), ts(n));
    }
    let ctx = ctx(log);
    let (mut all, mut cursor, mut pages) = (Vec::new(), None::<String>, 0);
    loop {
        let q = cursor.as_ref().map(|c| format!("cursor={c}"));
        let (status, body) = get(&ctx, "/dev/v1/events", q.as_deref());
        assert_eq!(status, 200, "{body}");
        assert_eq!(schema_errors("events", &body), Vec::<String>::new());
        let len = serde_json::to_vec(&body).expect("json").len();
        assert!(len <= PAGE_MAX_BYTES, "page is {len} B");
        all.extend(body["events"].as_array().cloned().unwrap_or_default());
        pages += 1;
        cursor = body["next"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert!(pages >= 2, "200 wide rows need more than one page");
    let got: Vec<String> = all
        .iter()
        .map(|e| e["ts"].as_str().unwrap_or("").to_owned())
        .collect();
    let want: Vec<String> = (0..200).map(ts).collect();
    assert_eq!(got, want, "every event once, oldest first");
}

#[test]
fn since_is_exclusive_and_checked() {
    let log = leaked();
    for n in 0..5 {
        log.push_at(Event::new(id("bg3-save"), Kind::Scan).count(n), ts(n));
    }
    let ctx = ctx(log);
    let (status, body) = get(&ctx, "/dev/v1/events", Some(&format!("since={}", ts(2))));
    assert_eq!(status, 200);
    let got: Vec<&str> = body["events"]
        .as_array()
        .map(|a| a.iter().filter_map(|e| e["ts"].as_str()).collect())
        .unwrap_or_default();
    assert_eq!(got, vec![ts(3).as_str(), ts(4).as_str()]);
    let (status, body) = get(&ctx, "/dev/v1/events", Some("since=yesterday"));
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["issues"][0]["path"], serde_json::json!(["since"]));
}

#[test]
fn a_cursor_from_another_route_or_dropped_is_400() {
    let log = leaked();
    for n in 0..3 {
        log.push_at(Event::new(id("bg3-save"), Kind::Scan), ts(n));
    }
    let ctx = ctx(log);
    for foreign in [encode_cursor(1), "ZXZlbnRz".to_owned(), "!!".to_owned()] {
        let (status, body) = get(&ctx, "/dev/v1/events", Some(&format!("cursor={foreign}")));
        assert_eq!(status, 400, "cursor {foreign} → {body}");
        assert_eq!(body["issues"][0]["path"], serde_json::json!(["cursor"]));
        assert_eq!(schema_errors("error", &body), Vec::<String>::new());
    }
    // An events cursor is refused by /logs.
    let (status, _) = get(&ctx, "/dev/v1/logs", Some(&format!("cursor={}", encode(1))));
    assert_eq!(status, 400, "/logs must refuse an events cursor");
    // A cursor whose event was evicted.
    for n in 0..CAPACITY {
        log.push_at(Event::new(id("bg3-save"), Kind::Scan), ts(n));
    }
    let (status, _) = get(
        &ctx,
        "/dev/v1/events",
        Some(&format!("cursor={}", encode(1))),
    );
    assert_eq!(status, 400, "a dropped event's cursor is refused");
    let (status, _) = get(
        &ctx,
        "/dev/v1/events",
        Some(&format!("cursor={}", encode(999_999))),
    );
    assert_eq!(status, 400, "a never-issued cursor is refused");
}

#[test]
fn ring_evicts_the_oldest_at_capacity() {
    let log = EventLog::default();
    for n in 0..CAPACITY + 7 {
        log.push_at(
            Event::new(id("bg3-save"), Kind::Scan).count(n),
            ts(n % 3600),
        );
    }
    assert_eq!(log.len(), CAPACITY);
    let body = log.page(None, None).expect("page");
    assert_eq!(
        body["events"][0]["count"],
        serde_json::json!(7),
        "first 7 dropped"
    );
}

#[test]
fn server_codes_map_through_the_enum() {
    assert_eq!(
        ErrorCode::from_server("invalid_payload"),
        ErrorCode::InvalidPayload
    );
    assert_eq!(
        ErrorCode::from_server("blob_refused"),
        ErrorCode::BlobRefused
    );
    for unknown in [
        "USER_DEACTIVATED",
        "INVALID_PAYLOAD",
        "brand_new",
        "",
        "invalid_payload ",
    ] {
        assert_eq!(
            ErrorCode::from_server(unknown),
            ErrorCode::Other,
            "{unknown:?}"
        );
    }
    let c = id("bg3-screenshot");
    let rej = |code: &str| UploadOutcome::Rejected {
        status: 422,
        code: code.into(),
        issue: Some("C:\\Users\\Jake\\save.lsv".into()),
    };
    let ev = for_outcome(c, &rej("USER_DEACTIVATED"), 0);
    assert_eq!(ev.error_code, Some(ErrorCode::Other));
    assert_eq!(ev.http_status, Some(422));
    assert_eq!(
        for_outcome(c, &rej(""), 0).error_code,
        None,
        "no code → omitted"
    );
    let held = UploadOutcome::RateLimited {
        wait: Duration::from_secs(5),
    };
    let ev = for_outcome(c, &held, 4);
    assert_eq!(
        (ev.kind, ev.http_status, ev.count),
        (Kind::UploadHeld, Some(429), Some(4))
    );
    let ev = for_outcome(c, &UploadOutcome::Unauthorized, 0);
    assert_eq!((ev.kind, ev.http_status), (Kind::LinkRequired, Some(401)));
    // A deferral reports the server's own code and status, never one
    // rebuilt from the display text (unknown_contract_version stays itself).
    let deferred = |status: u16, body: &str| match classify(status, None, body.as_bytes()) {
        Attempt::Done(o) => for_outcome(c, &o, 0),
        other => panic!("{status} {body} is not a deferral: {other:?}"),
    };
    let code = |c: &str| format!(r#"{{"code":"{c}"}}"#);
    for (status, c, want) in [
        (404, "unknown_contract", ErrorCode::UnknownContract),
        (
            404,
            "unknown_contract_version",
            ErrorCode::UnknownContractVersion,
        ),
        (
            409,
            "retired_contract_version",
            ErrorCode::RetiredContractVersion,
        ),
        (418, "TEAPOT", ErrorCode::Other),
    ] {
        let ev = deferred(status, &code(c));
        assert_eq!(
            (ev.kind, ev.http_status, ev.error_code),
            (Kind::UploadRetry, Some(status), Some(want)),
            "deferral {status} {c}"
        );
    }
    let ev = deferred(413, "<html>too large</html>");
    assert_eq!(
        (ev.http_status, ev.error_code),
        (Some(413), Some(ErrorCode::Other))
    );
    // The served row never carries the issue text or any key outside the schema.
    let log = leaked();
    log.push_at(for_outcome(c, &rej("hash_mismatch"), 0), ts(1));
    let body = log.page(None, None).expect("page");
    assert_eq!(schema_errors("events", &body), Vec::<String>::new());
    assert!(!body.to_string().contains("Jake"), "{body}");
}

#[test]
fn only_built_in_collectors_have_ids_and_status_is_bounded() {
    assert!(CollectorId::known("/Users/jake/WoW").is_none());
    assert!(CollectorId::known("Jake").is_none());
    let ev = Event::new(id("wow-ledgerlink"), Kind::UploadRetry).status(600);
    assert_eq!(ev.http_status, None, "out-of-range status is left out");
    assert_eq!(
        Event::new(id("bg3-save"), Kind::Scan)
            .count(usize::MAX)
            .count,
        Some(1_000_000)
    );
}

/// The real record points stamp rows through `now()` into the process ring,
/// and the served page is non-empty and schema-valid — so a timestamp-format
/// change cannot empty the route with every other test green.
#[test]
fn record_points_land_in_the_served_ring() {
    use crate::dev_bridge::events::{global, record, record_outcome, record_toggle};
    let has = |kind: &str| {
        let body = global().page(None, None).expect("page");
        assert_eq!(schema_errors("events", &body), Vec::<String>::new());
        let rows = body["events"].as_array().cloned().unwrap_or_default();
        rows.iter()
            .any(|r| r["collectorId"] == "bg3-save" && r["kind"] == kind)
    };
    record_toggle("bg3-save", true);
    assert!(
        has("collector-on"),
        "record_toggle left no collector-on row"
    );
    record("bg3-save", Kind::Scan, |e| e.count(2));
    assert!(has("scan"), "record left no scan row");
    record_outcome("bg3-save", &UploadOutcome::Unauthorized, 0);
    assert!(
        has("link-required"),
        "record_outcome left no link-required row"
    );
    let before = global().page(None, None).expect("page").to_string();
    record("/Users/x/save.lsv", Kind::Error, |e| e);
    assert!(!before.contains("/Users/x"));
    let after = global().page(None, None).expect("page").to_string();
    assert!(
        !after.contains("/Users/x"),
        "a non-collector id recorded a row"
    );
}
