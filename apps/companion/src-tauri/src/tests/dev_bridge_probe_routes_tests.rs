//! ROK-2191 `/dev/v1/probes` routes: POST → DevQueue.lua, answers ingested
//! from `LedgerLinkDB.dev.probes`, detail paging, DELETE, list paging,
//! route-bound cursors, the 409 and the 8 KiB body cap. Every body is checked
//! against the pinned schema.

use super::dev_bridge_unit_tests::schema_errors;
use crate::dev_bridge::auth::AuthPolicy;
use crate::dev_bridge::logbuf::LogBuffer;
use crate::dev_bridge::paging::{encode_cursor, PAGE_MAX_BYTES};
use crate::dev_bridge::probe_ingest::{check_name_bearing, refresh};
use crate::dev_bridge::probe_queue::extract;
use crate::dev_bridge::probe_store::ProbeStore;
use crate::dev_bridge::request_body::ReqBody;
use crate::dev_bridge::server::{ServerCtx, Snapshot};
use crate::dev_bridge::snapshot_routes::now_secs;
use crate::dev_bridge::token::sha256_hex;
use crate::dev_bridge::write_guard::queue_path;
use hyper::Method;
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CALLER: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 229);
const FLAVOR: &str = "_anniversary_";
const PROFESSIONS: &str =
    include_str!("../../../../../contracts/dev-bridge/v1/fixtures/probe-request--professions.json");
const RESULT: &str =
    include_str!("../../../../../contracts/dev-bridge/v1/fixtures/probe-result--professions.json");
const DISABLED: &str =
    include_str!("../../../../../contracts/dev-bridge/v1/fixtures/error--probes-disabled.json");
const FIXTURE_ID: &str = "01JBX7Q2M3N4P5R6S7T8V9W0XY";

fn token() -> String {
    format!("rlcdev_{}", "A".repeat(43))
}

struct Env {
    ctx: ServerCtx,
    wow: PathBuf,
    store: Arc<ProbeStore>,
}

/// A WoW folder with LedgerLink installed in FLAVOR and an account-scope
/// LedgerLink.lua (so `flavor: "auto"` resolves), and a probe store.
fn env(tag: &str, enabled: bool) -> Env {
    let wow = super::temp_dir(&format!("probe-routes-{tag}"));
    let q = queue_path(&wow, FLAVOR).expect("flavor");
    std::fs::create_dir_all(q.parent().expect("parent")).expect("install");
    write_answers(&wow, &[]);
    let store = Arc::new(ProbeStore::in_app_data(&wow.join("app")));
    let ctx = ServerCtx::with_logs(Box::leak(Box::new(LogBuffer::default())));
    ctx.set_snapshot(Snapshot {
        policy: AuthPolicy {
            allowlist: vec![CALLER],
            token_sha256: Some(sha256_hex(&token())),
        },
        wow_dir: wow.to_string_lossy().into_owned(),
        probes_enabled: enabled,
        snapshot_cache: None,
        probe_store: Some(store.clone()),
    });
    Env { ctx, wow, store }
}

/// `LedgerLinkDB.dev.probes = { [id] = "<json>" }` as the account file.
fn write_answers(wow: &Path, answers: &[(&str, String)]) {
    let dir = wow.join(FLAVOR).join("WTF/Account/ACC/SavedVariables");
    std::fs::create_dir_all(&dir).expect("mkdir sv");
    let entries: String = answers
        .iter()
        .map(|(id, j)| {
            let s = j.replace('\\', "\\\\").replace('"', "\\\"");
            format!("[\"{id}\"] = \"{s}\",\n")
        })
        .collect();
    let lua =
        format!("LedgerLinkDB = {{\n[\"dev\"] = {{\n[\"probes\"] = {{\n{entries}}},\n}},\n}}\n");
    std::fs::write(dir.join("LedgerLink.lua"), lua).expect("write sv");
}

impl Env {
    fn call(&self, m: Method, path: &str, q: Option<&str>, body: ReqBody) -> (u16, Value) {
        let auth = format!("Bearer {}", token());
        let r = self
            .ctx
            .route_full(IpAddr::V4(CALLER), &m, path, q, Some(&auth), &body);
        (r.status, r.body)
    }

    fn post(&self, body: &Value) -> (u16, Value) {
        let bytes = serde_json::to_vec(body).expect("json");
        self.call(Method::POST, "/dev/v1/probes", None, ReqBody::Bytes(bytes))
    }

    fn queued(&self, body: &Value) -> String {
        let (s, b) = self.post(body);
        assert_eq!(s, 202, "{b}");
        assert_eq!(schema_errors("probe-queued", &b), Vec::<String>::new());
        b["probeId"].as_str().expect("id").to_owned()
    }

    fn detail(&self, id: &str, q: Option<&str>) -> (u16, Value) {
        self.call(
            Method::GET,
            &format!("/dev/v1/probes/{id}"),
            q,
            ReqBody::None,
        )
    }

    fn delete(&self, id: &str) -> (u16, Value) {
        self.call(
            Method::DELETE,
            &format!("/dev/v1/probes/{id}"),
            None,
            ReqBody::None,
        )
    }

    /// Every detail page: 200, schema-valid, ≤ 16 KiB.
    fn detail_pages(&self, id: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let q = cursor.as_ref().map(|c| format!("cursor={c}"));
            let (s, b) = self.detail(id, q.as_deref());
            assert_eq!(s, 200, "{b}");
            assert_eq!(schema_errors("probe-detail", &b), Vec::<String>::new());
            let len = serde_json::to_vec(&b).expect("json").len();
            assert!(len <= PAGE_MAX_BYTES, "detail page is {len} B");
            cursor = b["next"].as_str().map(str::to_owned);
            out.push(b);
            if cursor.is_none() {
                return out;
            }
        }
        panic!("detail never ended");
    }
}

fn professions() -> Value {
    serde_json::from_str(PROFESSIONS).expect("fixture")
}

/// The professions result fixture, re-keyed to `id`.
fn result_for(id: &str) -> Value {
    let mut v: Value = serde_json::from_str(RESULT).expect("fixture");
    v["probeId"] = json!(id);
    v
}

fn queue_ids(wow: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(queue_path(wow, FLAVOR).expect("p")).expect("queue");
    let q: Value = serde_json::from_str(extract(&text).expect("bracket")).expect("json");
    assert_eq!(schema_errors("probe-queue", &q), Vec::<String>::new());
    let probes = q["probes"].as_array().expect("probes");
    probes
        .iter()
        .map(|p| p["probeId"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[test]
fn post_queues_the_professions_fixture_and_writes_devqueue() {
    let e = env("post", true);
    let (s, b) = e.post(&professions());
    assert_eq!(s, 202, "{b}");
    assert_eq!(schema_errors("probe-queued", &b), Vec::<String>::new());
    assert_eq!(b["flavor"], FLAVOR);
    assert_eq!(b["answersOn"], "next UI load, then next save");
    let id = b["probeId"].as_str().expect("id");
    assert_eq!(queue_ids(&e.wow), vec![id.to_owned()]);
    let (s, d) = e.detail(id, None);
    assert_eq!(s, 200);
    assert_eq!(schema_errors("probe-detail", &d), Vec::<String>::new());
    assert_eq!(
        (d["status"].clone(), d["next"].clone()),
        (json!("queued"), Value::Null)
    );
    assert_eq!(d["request"]["flavor"], FLAVOR);
}

#[test]
fn probes_disabled_is_409_on_post_and_without_a_store() {
    let e = env("disabled", false);
    let want: Value = serde_json::from_str(DISABLED).expect("fixture");
    assert_eq!(e.post(&professions()), (409, want.clone()));
    assert!(e.store.list(None).is_empty(), "nothing queued");
    // With a store, stored probes stay readable while probes are off.
    let (s, b) = e.call(Method::GET, "/dev/v1/probes", None, ReqBody::None);
    assert_eq!((s, schema_errors("probe-list", &b)), (200, vec![]));
    let mut snap = e.ctx.snapshot();
    snap.probe_store = None;
    e.ctx.set_snapshot(snap);
    let (s, b) = e.call(Method::GET, "/dev/v1/probes", None, ReqBody::None);
    assert_eq!((s, b), (409, want));
}

#[test]
fn invalid_bodies_are_400_with_issues() {
    let e = env("invalid", true);
    let mut bad = professions();
    bad["calls"][0]["fn"] = json!("SendChatMessage");
    let cases = [
        e.post(&bad),
        e.post(&json!({ "title": "" })),
        e.call(
            Method::POST,
            "/dev/v1/probes",
            None,
            ReqBody::Bytes(b"{nope".to_vec()),
        ),
        e.call(Method::POST, "/dev/v1/probes", None, ReqBody::TooBig),
    ];
    for (s, b) in &cases {
        assert_eq!(*s, 400, "{b}");
        assert_eq!(schema_errors("error", b), Vec::<String>::new());
        assert_eq!(b["error"], "invalid_request");
        assert!(!b["issues"].as_array().expect("issues").is_empty(), "{b}");
    }
    assert_eq!(cases[2].1["issues"][0]["code"], "invalid_type");
    assert_eq!(cases[3].1["issues"][0]["code"], "too_big");
    assert_eq!(cases[3].1["issues"][0]["path"], json!([]));
    assert!(e.store.list(None).is_empty());
}

#[test]
fn auth_is_required_before_any_probe_route() {
    let e = env("auth", true);
    let body = ReqBody::Bytes(serde_json::to_vec(&professions()).expect("json"));
    let r = e.ctx.route_full(
        IpAddr::V4(CALLER),
        &Method::POST,
        "/dev/v1/probes",
        None,
        None,
        &body,
    );
    assert_eq!((r.status, r.admitted), (401, false));
    assert!(e.store.list(None).is_empty());
}

#[test]
fn an_answer_round_trips_to_answered_with_its_result() {
    let e = env("answered", true);
    let id = e.queued(&professions());
    let result = result_for(&id);
    write_answers(&e.wow, &[(&id, result.to_string())]);
    let pages = e.detail_pages(&id);
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0]["status"], "answered");
    assert_eq!(pages[0]["result"], result);
    assert!(pages[0]["answeredAt"].is_string());
    assert!(
        queue_ids(&e.wow).is_empty(),
        "answered probe left the queue"
    );
}

#[test]
fn an_invalid_or_name_bearing_answer_is_rejected() {
    let e = env("rejected", true);
    let id = e.queued(&professions());
    let mut bad = result_for(&id);
    bad["calls"][0]["status"] = json!("bogus");
    let mut named_req = professions();
    named_req["calls"] = json!([{ "id": "me", "fn": "UnitName", "args": ["player"] }]);
    let named = e.queued(&named_req);
    let mut leak = result_for(&named);
    leak["calls"] = json!([{ "id": "me", "status": "ok", "values": ["Somebody"] }]);
    write_answers(
        &e.wow,
        &[(&id, bad.to_string()), (&named, leak.to_string())],
    );
    for (pid, why) in [
        (&id, "calls"),
        (&named, "name-bearing call me returned values"),
    ] {
        let (s, d) = e.detail(pid, None);
        assert_eq!(s, 200);
        assert_eq!(schema_errors("probe-detail", &d), Vec::<String>::new());
        assert_eq!(d["status"], "rejected");
        assert!(d.get("result").is_none(), "{d}");
        assert!(d["error"].as_str().expect("error").contains(why), "{d}");
    }
}

#[test]
fn name_bearing_calls_may_report_presence_only() {
    let req = json!({ "calls": [{ "id": "g", "fn": "UnitGUID" }, { "id": "x", "fn": "GetCVar" }] });
    let res = |values: Value| {
        json!({ "calls": [{ "id": "g", "status": "ok", "values": values },
            { "id": "x", "status": "ok", "values": ["free text", 1] }] })
    };
    assert_eq!(check_name_bearing(&req, &res(json!(["string"]))), Ok(()));
    assert_eq!(check_name_bearing(&req, &res(json!([]))), Ok(()));
    for v in [
        json!(["Player-1-0000ABCD"]),
        json!(["nil", "nil"]),
        json!([1]),
    ] {
        assert!(check_name_bearing(&req, &res(v)).is_err());
    }
    let its = json!({ "calls": [{ "id": "g", "status": "ok",
        "iterations": [{ "i": 1, "status": "ok", "values": ["Name"] }] }] });
    assert!(check_name_bearing(&req, &its).is_err());
}

#[test]
fn a_large_result_pages_by_item_and_reassembles() {
    let e = env("large", true);
    let id = e.queued(&professions());
    let mut result = result_for(&id);
    let big = "x".repeat(480);
    let its: Vec<Value> = (1..=20)
        .map(|i| json!({ "i": i, "status": "ok", "values": [big, big, big, big, big] }))
        .collect();
    result["calls"][2]["iterations"] = Value::Array(its);
    write_answers(&e.wow, &[(&id, result.to_string())]);
    let pages = e.detail_pages(&id);
    assert!(pages.len() > 3, "{} pages", pages.len());
    let mut calls: Vec<Value> = Vec::new();
    for p in &pages {
        assert_eq!(p["status"], "answered");
        for c in p["result"]["calls"].as_array().expect("calls") {
            match calls.iter_mut().find(|x| x["id"] == c["id"]) {
                Some(prev) => {
                    let more = c["iterations"].as_array().expect("its").clone();
                    prev["iterations"].as_array_mut().expect("its").extend(more);
                }
                None => calls.push(c.clone()),
            }
        }
    }
    assert_eq!(Value::Array(calls), result["calls"]);
}

#[test]
fn a_probe_expires_after_its_ttl() {
    let e = env("expired", true);
    let id = e.queued(&professions());
    refresh(&e.store, &e.ctx.snapshot().wow_dir, now_secs() + 73 * 3600);
    let (s, d) = e.detail(&id, None);
    assert_eq!((s, d["status"].clone()), (200, json!("expired")));
    assert!(queue_ids(&e.wow).is_empty());
    let (s, b) = e.delete(&id);
    assert_eq!(
        (s, b["issues"][0]["message"].clone()),
        (400, json!("probe is expired"))
    );
}

#[test]
fn an_answer_read_after_the_ttl_is_still_applied() {
    let e = env("late-answer", true);
    let id = e.queued(&professions());
    write_answers(&e.wow, &[(&id, result_for(&id).to_string())]);
    refresh(&e.store, &e.ctx.snapshot().wow_dir, now_secs() + 73 * 3600);
    let (s, d) = e.detail(&id, None);
    assert_eq!((s, d["status"].clone()), (200, json!("answered")));
}

#[test]
fn a_g_prefixed_name_bearing_fn_is_held_to_presence_only() {
    let req = json!({ "calls": [{ "id": "me", "fn": "_G.UnitName" }] });
    let leak = json!({ "calls": [{ "id": "me", "status": "ok", "values": ["Somebody"] }] });
    assert!(check_name_bearing(&req, &leak).is_err());
    let ok = json!({ "calls": [{ "id": "me", "status": "ok", "values": ["function"] }] });
    assert_eq!(check_name_bearing(&req, &ok), Ok(()));
}

#[test]
fn delete_cancels_idempotently_and_refuses_finished_probes() {
    let e = env("delete", true);
    let id = e.queued(&professions());
    let want = json!({ "probeId": id, "status": "cancelled" });
    for _ in 0..2 {
        let (s, b) = e.delete(&id);
        assert_eq!((s, &b), (200, &want));
        assert_eq!(schema_errors("probe-cancelled", &b), Vec::<String>::new());
    }
    assert!(queue_ids(&e.wow).is_empty());
    let done = e.queued(&professions());
    write_answers(&e.wow, &[(&done, result_for(&done).to_string())]);
    let (s, b) = e.delete(&done);
    assert_eq!(s, 400);
    assert_eq!(
        b["issues"][0],
        json!({ "path": [], "code": "custom", "message": "probe is answered" })
    );
    assert_eq!(e.delete(FIXTURE_ID).0, 404);
    assert_eq!(e.delete("not-an-id").0, 404);
    assert_eq!(e.detail("not-an-id", None).0, 404);
    assert_eq!(e.detail(FIXTURE_ID, None).0, 404);
}

#[test]
fn list_filters_by_status_pages_newest_first_and_binds_cursors() {
    let e = env("list", true);
    let mut ids = Vec::new();
    for _ in 0..101 {
        let id = e.queued(&professions());
        assert_eq!(e.delete(&id).0, 200);
        ids.push(id);
    }
    let last = e.queued(&professions());
    let list = |q: &str| e.call(Method::GET, "/dev/v1/probes", Some(q), ReqBody::None);
    let (s, b) = list("status=queued");
    assert_eq!((s, b["probes"].as_array().map(Vec::len)), (200, Some(1)));
    assert_eq!(b["probes"][0]["probeId"], json!(last));
    let (s, p1) = list("status=cancelled");
    assert_eq!((s, schema_errors("probe-list", &p1)), (200, vec![]));
    assert_eq!(p1["probes"].as_array().map(Vec::len), Some(100));
    assert_eq!(p1["probes"][0]["probeId"], json!(ids[100]));
    let cursor = p1["next"].as_str().expect("next").to_owned();
    let (s, p2) = list(&format!("status=cancelled&cursor={cursor}"));
    assert_eq!((s, schema_errors("probe-list", &p2)), (200, vec![]));
    assert_eq!(p2["probes"][0]["probeId"], json!(ids[0]));
    assert_eq!(p2["next"], Value::Null);
    let (s, b) = list("status=bogus");
    assert_eq!(
        (s, b["issues"][0]["path"].clone()),
        (400, json!(["status"]))
    );
    // A list cursor is refused on detail, and other routes' cursors on both.
    let foreign = encode_cursor(0);
    for (s, b) in [
        e.detail(&last, Some(&format!("cursor={cursor}"))),
        list(&format!("cursor={foreign}")),
        e.detail(&last, Some(&format!("cursor={foreign}"))),
    ] {
        assert_eq!(
            (s, b["issues"][0]["path"].clone()),
            (400, json!(["cursor"])),
            "{b}"
        );
    }
}

#[test]
fn a_detail_cursor_is_bound_to_its_probe_and_route() {
    let e = env("detail-cursor", true);
    let id = e.queued(&professions());
    let other = e.queued(&professions());
    let mut result = result_for(&id);
    let big = "y".repeat(480);
    let its: Vec<Value> = (1..=12)
        .map(|i| json!({ "i": i, "status": "ok", "values": [big, big, big, big, big] }))
        .collect();
    result["calls"][2]["iterations"] = Value::Array(its);
    write_answers(&e.wow, &[(&id, result.to_string())]);
    let (_, first) = e.detail(&id, None);
    let next = first["next"].as_str().expect("paged").to_owned();
    let q = format!("cursor={next}");
    assert_eq!(e.detail(&other, Some(&q)).0, 400);
    let (s, b) = e.call(Method::GET, "/dev/v1/probes", Some(&q), ReqBody::None);
    assert_eq!(
        (s, b["issues"][0]["path"].clone()),
        (400, json!(["cursor"]))
    );
}

/// Over HTTPS: an 8 KiB+ body is refused with 400 `too_big` (never parsed),
/// and an unauthenticated POST is a 401 before any body is read.
#[tokio::test]
async fn an_oversized_body_is_refused_over_the_wire() {
    use crate::config::AppConfig;
    use crate::dev_bridge::settings::DevBridgeConfig;
    use crate::dev_bridge::tls::TlsMaterial;
    use crate::dev_bridge::{DevBridge, TLS_FILE};
    use std::net::SocketAddrV4;
    let dir = super::temp_dir("probe-routes-live");
    let cfg = AppConfig {
        dev_bridge: DevBridgeConfig {
            enabled: true,
            allowlist: vec!["127.0.0.1".into()],
            token_sha256: Some(sha256_hex(&token())),
            probes_enabled: true,
            ..DevBridgeConfig::default()
        },
        ..AppConfig::default()
    };
    let bridge = DevBridge::new(dir.join(TLS_FILE), Some(&dir));
    bridge.refresh(&cfg);
    let mut snap = bridge.ctx().snapshot();
    snap.probe_store = Some(Arc::new(ProbeStore::in_app_data(&dir)));
    bridge.ctx().set_snapshot(snap);
    bridge
        .listen(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("listen");
    let addr = bridge.listening_addr().await.expect("listening");
    let cert = TlsMaterial::load_or_create(&dir.join(TLS_FILE), Ipv4Addr::LOCALHOST)
        .expect("cert")
        .cert()
        .expect("der");
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_der(&cert).expect("cert"))
        .tls_built_in_root_certs(false)
        .no_proxy()
        .build()
        .expect("client");
    let url = format!("https://{addr}/dev/v1/probes");
    let big = format!("{{\"title\":\"{}\"}}", "x".repeat(9_000));
    let send = |auth: Option<String>| {
        let mut req = client.post(&url).body(big.clone());
        if let Some(a) = auth {
            req = req.header("Authorization", a);
        }
        req.send()
    };
    let res = send(Some(format!("Bearer {}", token())))
        .await
        .expect("send");
    assert_eq!(res.status().as_u16(), 400);
    let body: Value = res.json().await.expect("json");
    assert_eq!(schema_errors("error", &body), Vec::<String>::new());
    assert_eq!(body["issues"][0]["code"], "too_big");
    let res = send(None).await.expect("send");
    assert_eq!(res.status().as_u16(), 401);
    bridge.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}
