//! ROK-2072 D9 / AC-9: the 30 s poll ingests a changed SavedVariables file,
//! and nothing is ingested (or served) while the bridge is off.

use super::temp_dir;
use crate::config::AppConfig;
use crate::dev_bridge::logbuf::LogBuffer;
use crate::dev_bridge::paging::parse_query;
use crate::dev_bridge::server::{ServerCtx, Snapshot};
use crate::dev_bridge::snapshot_cache::{character_key, SnapshotCache};
use crate::dev_bridge::snapshot_routes::poll_loop;
use crate::dev_bridge::{routes, DevBridge, TLS_FILE};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const FLAVOR: &str = "_anniversary_";
const G1: &str = "Player-0000-0A000001";

/// The minimal fixture with a distinct build (a distinct digest).
fn variant(i: i64) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../contracts/dev-bridge/v1/fixtures/ledgerlink-snapshot--minimal.json");
    let mut v: Value =
        serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("fixture json");
    v["client"]["build"] = json!(format!("{}", 1000 + i));
    v
}

/// Write `LedgerLinkDB.dev.snapshots[G1] = v` as the account-scope file,
/// stamping `mtime` so the poll's mtime gate sees the change.
fn write_sv(wow: &Path, v: &Value, mtime: SystemTime) {
    let dir = wow.join(FLAVOR).join("WTF/Account/ACCT1/SavedVariables");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let s = v.to_string().replace('\\', "\\\\").replace('"', "\\\"");
    let lua = format!(
        "LedgerLinkDB = {{\n[\"dev\"] = {{\n[\"snapshots\"] = {{\n[\"{G1}\"] = \"{s}\",\n}},\n}},\n}}\n"
    );
    let path = dir.join("LedgerLink.lua");
    std::fs::write(&path, lua).expect("write sv");
    let f = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open");
    f.set_modified(mtime).expect("set mtime");
}

fn row_count(cache: &SnapshotCache) -> usize {
    cache.rows(FLAVOR, &character_key(G1)).len()
}

/// One poll_loop run: its first tick fires at once; wait (bounded) until the
/// cache holds `want` rows, then stop the loop.
async fn poll_until(ctx: &Arc<ServerCtx>, cache: &SnapshotCache, want: usize) -> usize {
    let task = tokio::spawn(poll_loop(ctx.clone()));
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while row_count(cache) < want {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    task.abort();
    row_count(cache)
}

const BOUND: Duration = Duration::from_secs(3);

/// Run poll_loop until `cache` holds a character or `bound` passes.
async fn ingested_within(ctx: Arc<ServerCtx>, cache: &SnapshotCache, bound: Duration) -> bool {
    let task = tokio::spawn(poll_loop(ctx));
    let hit = tokio::time::timeout(bound, async {
        while cache.characters(FLAVOR).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    task.abort();
    let _ = task.await;
    hit
}

#[test]
fn a_file_that_failed_to_parse_is_retried_at_the_same_mtime() {
    let dir = temp_dir("snap-mtime-retry");
    let wow = dir.join("wow");
    let cache = SnapshotCache::new(dir.join("cache"));
    let t0 = SystemTime::now() - Duration::from_secs(60);
    write_sv(&wow, &variant(1), t0);
    let path = wow
        .join(FLAVOR)
        .join("WTF/Account/ACCT1/SavedVariables/LedgerLink.lua");
    let full = std::fs::read(&path).expect("read sv");
    // Half-written (WoW writes SavedVariables on logout): an unparsable file.
    std::fs::write(&path, &full[..full.len() / 2]).expect("truncate");
    let f = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open");
    f.set_modified(t0).expect("set mtime");
    assert_eq!(cache.ingest_file(FLAVOR, &path, 1_793_000_000), 0);

    write_sv(&wow, &variant(1), t0);
    assert_eq!(
        cache.ingest_file(FLAVOR, &path, 1_793_000_000),
        1,
        "the complete file at the same mtime must be read, not skipped as seen"
    );
}

#[tokio::test]
async fn the_poll_ingests_a_changed_saved_variables_file() {
    let dir = temp_dir("snap-poll-change");
    let wow = dir.join("wow");
    let cache = Arc::new(SnapshotCache::new(dir.join("cache")));
    let ctx = Arc::new(ServerCtx::default());
    ctx.set_snapshot(Snapshot {
        wow_dir: wow.to_string_lossy().into_owned(),
        snapshot_cache: Some(cache.clone()),
        ..Snapshot::default()
    });
    let t0 = SystemTime::now() - Duration::from_secs(60);
    write_sv(&wow, &variant(1), t0);
    assert_eq!(poll_until(&ctx, &cache, 1).await, 1, "first poll ingests");

    write_sv(&wow, &variant(2), t0 + Duration::from_secs(5));
    assert_eq!(
        poll_until(&ctx, &cache, 2).await,
        2,
        "a changed file is ingested as a new row"
    );
}

#[tokio::test]
async fn nothing_is_ingested_or_served_while_the_bridge_is_off() {
    let dir = temp_dir("snap-poll-off");
    let wow = dir.join("wow");
    write_sv(&wow, &variant(1), SystemTime::now());
    let app = dir.join("app");
    let bridge = DevBridge::new(dir.join(TLS_FILE), Some(&app));
    let cfg = AppConfig {
        wow_dir: wow.to_string_lossy().into_owned(),
        ..AppConfig::default()
    };
    assert!(!cfg.dev_bridge.enabled, "toggle off");
    bridge.reconcile(&cfg).await;
    let served = bridge.ctx().snapshot();
    assert!(
        served.snapshot_cache.is_none(),
        "the off bridge hands out no cache"
    );

    // Control: the same poll with a cache under `app` ingests well inside
    // the bound the off run waits for below.
    let control_dir = dir.join("control");
    let on_ctx = Arc::new(ServerCtx::default());
    on_ctx.set_snapshot(Snapshot {
        snapshot_cache: Some(Arc::new(SnapshotCache::in_app_data(&control_dir))),
        ..served.clone()
    });
    let control = SnapshotCache::in_app_data(&control_dir);
    assert!(
        ingested_within(on_ctx, &control, BOUND).await,
        "control: an on bridge's poll ingests within the bound"
    );

    let ctx = Arc::new(ServerCtx::default());
    ctx.set_snapshot(served.clone());
    let on_disk = SnapshotCache::in_app_data(&app);
    assert!(
        !ingested_within(ctx, &on_disk, BOUND).await,
        "the off bridge's poll ingested into the app-data cache"
    );
    let logs = LogBuffer::default();
    let q = parse_query(Some(&format!("flavor={FLAVOR}")));
    let res = routes::get(&served, "/dev/v1/ledgerlink/snapshot", &q, None, &logs);
    assert_eq!(res.map(|r| r.0), Some(404), "no read-through while off");
    assert_eq!(
        on_disk.characters(FLAVOR),
        Vec::<String>::new(),
        "nothing ingested while off"
    );
}
