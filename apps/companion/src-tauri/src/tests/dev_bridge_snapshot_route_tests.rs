//! ROK-2072 PR-C (AC-10): `/dev/v1/ledgerlink/snapshot` + `/history` —
//! bodies valid against the pinned `$defs` and ≤ 16 KiB, multi-page
//! reassembly, cursors across new rows and pruning, 400/404, read-through
//! from `LedgerLink.lua`. Synthetic snapshots only (fake GUIDs, no names).

use super::dev_bridge_unit_tests::schema_errors;
use super::temp_dir;
use crate::dev_bridge::logbuf::LogBuffer;
use crate::dev_bridge::paging::{parse_query, PAGE_MAX_BYTES};
use crate::dev_bridge::routes;
use crate::dev_bridge::server::Snapshot;
use crate::dev_bridge::snapshot_cache::{character_key, SnapshotCache};
use base64::Engine;
use rl_collectors::DevSnapshotEntry;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const FLAVOR: &str = "_anniversary_";
const G1: &str = "Player-0000-0A000001";
const G2: &str = "Player-0000-0A000002";
const PAGE: &str = "/dev/v1/ledgerlink/snapshot";
const HISTORY: &str = "/dev/v1/ledgerlink/snapshot/history";
const NOW: i64 = 1_793_000_000;

fn fixture(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../contracts/dev-bridge/v1/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(p).expect(name)).expect(name)
}

/// The minimal fixture with a distinct build (a distinct digest) and time.
fn variant(i: i64) -> Value {
    let mut v = fixture("ledgerlink-snapshot--minimal.json");
    v["client"]["build"] = json!(format!("{}", 1000 + i));
    v["capturedAt"] = json!(rl_collectors::rfc3339_utc_from_unix_seconds(NOW + i));
    v
}

struct Env {
    wow: PathBuf,
    cache: Arc<SnapshotCache>,
}

impl Env {
    fn new(name: &str) -> Self {
        let dir = temp_dir(name);
        let wow = dir.join("wow");
        std::fs::create_dir_all(wow.join(FLAVOR)).expect("mkdir");
        let cache = Arc::new(SnapshotCache::new(dir.join("cache")));
        Self { wow, cache }
    }

    fn add(&self, guid: &str, v: &Value, now: i64) {
        let e = DevSnapshotEntry { guid: guid.into(), json: v.to_string() };
        assert_eq!(self.cache.ingest(FLAVOR, &[e], now), 1, "ingested");
    }

    fn snap(&self, with_cache: bool) -> Snapshot {
        Snapshot {
            wow_dir: self.wow.to_string_lossy().into_owned(),
            snapshot_cache: with_cache.then(|| self.cache.clone()),
            ..Snapshot::default()
        }
    }

    fn get(&self, path: &str, q: &str) -> (u16, Value) {
        let logs = LogBuffer::default();
        routes::get(&self.snap(true), path, &parse_query(Some(q)), None, &logs).expect("routed")
    }
}

fn ok_body(env: &Env, path: &str, q: &str, def: &str) -> Value {
    let (status, body) = env.get(path, q);
    assert_eq!(status, 200, "{path}?{q} → {body}");
    assert_eq!(schema_errors(def, &body), Vec::<String>::new(), "{path}?{q}");
    let len = serde_json::to_vec(&body).unwrap().len();
    assert!(len <= PAGE_MAX_BYTES, "{path}?{q}: {len} bytes");
    body
}

fn issue_at(body: &Value) -> &str {
    body["issues"][0]["path"][0].as_str().unwrap_or("")
}

/// Every page of the snapshot endpoint for `base`.
fn all_pages(env: &Env, base: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for _ in 0..50 {
        let q = match out.last().and_then(|b| b["next"].as_str()) {
            Some(c) => format!("{base}&cursor={c}"),
            None if out.is_empty() => base.to_owned(),
            None => return out,
        };
        out.push(ok_body(env, PAGE, &q, "ledgerlink-snapshot-page"));
    }
    panic!("no last page in 50");
}

fn reassemble(pages: &[Value]) -> Value {
    let first = pages.first().expect("a page");
    let mut whole = first["snapshot"].clone();
    for k in ["calls", "apis", "sections"] {
        let all: Vec<Value> = pages
            .iter()
            .flat_map(|p| p["snapshot"][k].as_array().cloned().unwrap_or_default())
            .collect();
        whole[k] = json!(all);
    }
    whole
}

#[test]
fn read_through_serves_a_saved_snapshot_in_valid_pages_that_reassemble() {
    let env = Env::new("snap-route-read-through");
    let stored = fixture("ledgerlink-snapshot--max-size.json");
    let lua_str = stored.to_string().replace('\\', "\\\\").replace('"', "\\\"");
    let sv_dir = env.wow.join(FLAVOR).join("WTF/Account/ACCT1/SavedVariables");
    std::fs::create_dir_all(&sv_dir).expect("mkdir");
    let lua = format!(
        "LedgerLinkDB = {{\n[\"dev\"] = {{\n[\"snapshots\"] = {{\n[\"{G1}\"] = \"{lua_str}\",\n}},\n}},\n}}\n"
    );
    std::fs::write(sv_dir.join("LedgerLink.lua"), lua).expect("write sv");
    let pages = all_pages(&env, &format!("flavor={FLAVOR}"));
    assert!(pages.len() >= 3, "a 48 KiB snapshot spans pages: {}", pages.len());
    assert_eq!(reassemble(&pages), stored, "pages concatenate to the stored object");
    let first = &pages[0];
    assert_eq!(first["characterKey"], json!(character_key(G1)));
    assert_eq!((first["latest"].clone(), first["seenCount"].clone()), (json!(true), json!(1)));
}

#[test]
fn a_cursor_survives_a_new_row_and_a_pruned_or_bad_cursor_is_400() {
    let env = Env::new("snap-route-cursor");
    env.add(G1, &fixture("ledgerlink-snapshot--max-size.json"), NOW);
    let first = ok_body(&env, PAGE, "", "ledgerlink-snapshot-page");
    let cursor = first["next"].as_str().expect("multi-page").to_owned();
    env.add(G1, &variant(60), NOW + 60);
    let second = ok_body(&env, PAGE, &format!("cursor={cursor}"), "ledgerlink-snapshot-page");
    assert_eq!(second["snapshotId"], first["snapshotId"], "same row after a new one");
    assert_eq!(second["latest"], json!(false));
    let latest = ok_body(&env, PAGE, "", "ledgerlink-snapshot-page");
    assert_ne!(latest["snapshotId"], first["snapshotId"], "default is the newest row");
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    for bad in [b64.encode("s01ZZZZZZZZZZZZZZZZZZZZZZZ:3"), "!!".into()] {
        let (status, body) = env.get(PAGE, &format!("cursor={bad}"));
        assert_eq!((status, issue_at(&body)), (400, "cursor"), "{bad} → {body}");
        assert!(schema_errors("error", &body).is_empty(), "{body}");
    }
}

#[test]
fn unknown_or_ambiguous_params_are_400_and_nothing_cached_is_404() {
    let env = Env::new("snap-route-errors");
    let logs = LogBuffer::default();
    let off = routes::get(&env.snap(false), PAGE, &parse_query(None), None, &logs);
    assert_eq!(off.map(|r| r.0), Some(404), "no cache handle");
    assert_eq!(env.get(PAGE, "").0, 404, "nothing cached");
    assert_eq!(env.get(HISTORY, "").0, 404, "no history");
    env.add(G1, &variant(1), NOW);
    let (s, b) = env.get(PAGE, "flavor=_nope_");
    assert_eq!((s, issue_at(&b)), (400, "flavor"));
    let (s, b) = env.get(PAGE, &format!("character={}", "0".repeat(64)));
    assert_eq!((s, issue_at(&b)), (400, "character"));
    assert_eq!(env.get(PAGE, "at=01NOPE").0, 404, "unknown at");
    env.add(G2, &variant(2), NOW + 1);
    let (s, b) = env.get(PAGE, "");
    assert_eq!((s, issue_at(&b)), (400, "character"), "two cached, none named");
    let q = format!("character={}", character_key(G2));
    let body = ok_body(&env, PAGE, &q, "ledgerlink-snapshot-page");
    assert_eq!(body["characterKey"], json!(character_key(G2)));
}

#[test]
fn history_is_newest_first_pages_by_snapshot_id_and_filters_by_character() {
    let env = Env::new("snap-route-history");
    for i in 0..40 {
        env.add(G1, &variant(i), NOW + i);
    }
    env.add(G2, &variant(100), NOW + 100);
    let mut ids: Vec<String> = Vec::new();
    let mut q = String::new();
    for _ in 0..20 {
        let body = ok_body(&env, HISTORY, &q, "ledgerlink-snapshot-history");
        for e in body["entries"].as_array().expect("entries") {
            ids.push(e["snapshotId"].as_str().expect("id").to_owned());
        }
        match body["next"].as_str() {
            Some(c) => q = format!("cursor={c}"),
            None => break,
        }
    }
    assert_eq!(ids.len(), 41, "every row once across pages");
    assert!(ids.windows(2).all(|w| w[0] > w[1]), "strictly newest first");
    let q = format!("character={}", character_key(G2));
    let only = ok_body(&env, HISTORY, &q, "ledgerlink-snapshot-history");
    let entries = only["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["latest"], json!(true));
    assert_eq!(entries[0]["clientBuild"], json!("1100"));
    let at = entries[0]["snapshotId"].as_str().expect("id");
    let page = ok_body(&env, PAGE, &format!("{q}&at={at}"), "ledgerlink-snapshot-page");
    assert_eq!(page["snapshotId"], json!(at));
}

#[test]
fn routes_never_take_a_request_value_as_a_path() {
    let env = Env::new("snap-route-paths");
    env.add(G1, &variant(1), NOW);
    let (s, _) = env.get(PAGE, "flavor=../../etc");
    assert_eq!(s, 400);
    let (s, _) = env.get(PAGE, "character=../../../../etc/passwd");
    assert_eq!(s, 400);
    assert!(!Path::new(&env.wow).join("etc").exists());
}
