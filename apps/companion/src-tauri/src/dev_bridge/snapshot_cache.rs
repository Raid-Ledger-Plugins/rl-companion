//! LedgerLink dev snapshot cache (ROK-2072 §6, AC-9): latest + 7 days of
//! history per flavor + character, on local disk only — this module has no
//! path to the upload client. One JSON Lines file per character, oldest
//! first: `<app_data_dir>/dev-bridge/snapshots/<flavor>/<characterKey>.jsonl`.
//! Every write goes through `write_guard::guarded_cache_write`.

use super::logbuf;
use super::snapshot_validate::validate_snapshot;
use super::token::sha256_hex;
use super::write_guard::{cache_file, guarded_cache_remove, guarded_cache_write};
use rl_collectors::{rfc3339_utc_from_unix_seconds, DevSnapshotEntry};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Under the app-data dir.
pub const CACHE_DIR: [&str; 2] = ["dev-bridge", "snapshots"];
pub const RETAIN_SECS: i64 = 7 * 24 * 3600;
pub const MAX_ROWS_PER_FILE: usize = 200;
pub const MAX_CACHE_BYTES: u64 = 32 * 1024 * 1024;
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// One history row (one JSONL line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub snapshot_id: String,
    pub received_at: String,
    pub captured_at: String,
    pub last_captured_at: String,
    pub seen_count: u64,
    pub digest: String,
    pub snapshot: Value,
}

#[derive(Debug)]
pub struct SnapshotCache {
    root: PathBuf,
    lock: Mutex<()>,
    /// Last-read mtime per `LedgerLink.lua`, so an unchanged file is not re-parsed.
    mtimes: Mutex<HashMap<PathBuf, SystemTime>>,
}

/// `characterKey`: the same hash `sv-parsed` reports as `guidHash`.
pub fn character_key(guid: &str) -> String {
    sha256_hex(guid)
}

/// `value` with every object's keys in sorted order (whatever serde_json's
/// `preserve_order` feature says — it is off in this build, so `Map` is a
/// `BTreeMap` already; this keeps the digest stable if that ever changes).
fn canonical(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonical(&o[k])))
                    .collect::<Map<_, _>>(),
            )
        }
        other => other.clone(),
    }
}

/// D10: sha256 of the canonical snapshot JSON without `capturedAt`.
pub fn digest(snapshot: &Value) -> String {
    let mut v = canonical(snapshot);
    if let Some(o) = v.as_object_mut() {
        o.remove("capturedAt");
    }
    sha256_hex(&serde_json::to_string(&v).unwrap_or_default())
}

/// 26 chars of Crockford base32: 48-bit ms time + 80 random bits (ULID layout).
pub fn new_snapshot_id(now_ms: u64) -> String {
    const ALPHA: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut rnd = [0u8; 10];
    let _ = getrandom::getrandom(&mut rnd);
    let mut n: u128 = u128::from(now_ms & 0xFFFF_FFFF_FFFF) << 80;
    for (i, b) in rnd.iter().enumerate() {
        n |= u128::from(*b) << (8 * (9 - i));
    }
    (0..26)
        .rev()
        .map(|i| ALPHA[((n >> (5 * i)) & 31) as usize] as char)
        .collect()
}

/// The ms time part of a [`new_snapshot_id`] (its first 10 chars).
pub fn id_ms(id: &str) -> Option<u64> {
    const ALPHA: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    id.get(..10)?
        .bytes()
        .try_fold(0u64, |n, c| {
            let d = ALPHA.iter().position(|a| *a == c)?;
            Some((n << 5) | d as u64)
        })
        .map(|n| n & 0xFFFF_FFFF_FFFF)
}

/// `now` (seconds) in ms, with the clock's current sub-second part, so rows
/// written in the same second still sort in write order.
fn now_ms(now: i64) -> u64 {
    let sub = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_millis()));
    u64::try_from(now).unwrap_or(0) * 1000 + sub
}

fn row_bytes(r: &Row) -> u64 {
    serde_json::to_string(r).map_or(0, |s| s.len() as u64 + 1)
}

/// One warn line per kind of parser rejection (never the key: it is a GUID).
fn log_skipped(skipped: &[rl_collectors::SkippedEntry]) {
    let mut kinds: Vec<String> = skipped.iter().map(|s| format!("{:?}", s.reason)).collect();
    kinds.sort();
    kinds.dedup();
    if !kinds.is_empty() {
        let msg = format!(
            "dev snapshot: {} entries skipped ({})",
            skipped.len(),
            kinds.join(", ")
        );
        logbuf::log("warn", "dev_bridge", &msg);
    }
}

fn secs(ts: &str) -> i64 {
    logbuf::parse_utc(ts).map_or(i64::MIN, |(s, _)| s)
}

fn is_key(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl SnapshotCache {
    /// A cache rooted at `<app_data_dir>/dev-bridge/snapshots`.
    pub fn in_app_data(app_data_dir: &Path) -> Self {
        Self::new(
            CACHE_DIR
                .iter()
                .fold(app_data_dir.to_path_buf(), |p, s| p.join(s)),
        )
    }

    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            lock: Mutex::new(()),
            mtimes: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Rows of one file, oldest first (unreadable lines are dropped).
    pub fn rows(&self, flavor: &str, key: &str) -> Vec<Row> {
        let Ok(path) = cache_file(&self.root, flavor, key) else {
            return Vec::new();
        };
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// The cached `characterKey`s of `flavor`, sorted.
    pub fn characters(&self, flavor: &str) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.root.join(flavor)) else {
            return Vec::new();
        };
        let mut out: Vec<String> = rd
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".jsonl").map(str::to_owned))
            .filter(|k| is_key(k))
            .collect();
        out.sort();
        out
    }

    fn flavors(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out: Vec<String> = rd
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| super::health::is_flavor(n))
            .collect();
        out.sort();
        out
    }

    fn write(&self, flavor: &str, key: &str, rows: &[Row]) {
        let res = if rows.is_empty() {
            guarded_cache_remove(&self.root, flavor, key)
        } else {
            let mut buf = String::new();
            for r in rows {
                buf.push_str(&serde_json::to_string(r).unwrap_or_default());
                buf.push('\n');
            }
            guarded_cache_write(&self.root, flavor, key, buf.as_bytes())
        };
        if let Err(e) = res {
            logbuf::log(
                "warn",
                "dev_bridge",
                &format!("dev snapshot cache write failed: {e}"),
            );
        }
    }

    /// Validate + merge `entries` (one file's `dev.snapshots`) into the
    /// cache. Returns how many files changed; prunes when any did.
    pub fn ingest(&self, flavor: &str, entries: &[DevSnapshotEntry], now: i64) -> usize {
        if !super::health::is_flavor(flavor) {
            return 0;
        }
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut changed = 0;
        for e in entries {
            let key = character_key(&e.guid);
            let parsed = serde_json::from_str::<Value>(&e.json).map_err(|e| e.to_string());
            let snap = match parsed.and_then(|v| validate_snapshot(&v).map(|()| v)) {
                Ok(v) => v,
                Err(why) => {
                    let msg = format!("dev snapshot {} rejected: {why}", &key[..8]);
                    logbuf::log("warn", "dev_bridge", &msg);
                    continue;
                }
            };
            let mut rows = self.rows(flavor, &key);
            if merge(&mut rows, snap, now) {
                prune_rows(&mut rows, now);
                self.write(flavor, &key, &rows);
                changed += 1;
            }
        }
        if changed > 0 {
            self.prune_locked(now);
        }
        changed
    }

    /// Read `LedgerLinkDB.dev.snapshots` from one account-scope
    /// `LedgerLink.lua` of `flavor` and ingest it; a file whose mtime has not
    /// moved since the last read is skipped.
    pub fn ingest_file(&self, flavor: &str, path: &Path, now: i64) -> usize {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let seen =
            |m: &HashMap<PathBuf, SystemTime>| mtime.is_some() && m.get(path) == mtime.as_ref();
        if seen(&self.mtimes.lock().unwrap_or_else(|p| p.into_inner())) {
            return 0;
        }
        let Ok(bytes) = std::fs::read(path) else {
            return 0;
        };
        match rl_collectors::parse_ledgerlink_dev_snapshots(&bytes) {
            Ok(d) => {
                // Marked seen only once it parsed: a half-written file (WoW
                // writes SavedVariables on logout) is retried next tick.
                if let Some(t) = mtime {
                    let mut m = self.mtimes.lock().unwrap_or_else(|p| p.into_inner());
                    m.insert(path.to_path_buf(), t);
                }
                log_skipped(&d.skipped);
                self.ingest(flavor, &d.entries, now)
            }
            Err(e) => {
                logbuf::log(
                    "warn",
                    "dev_bridge",
                    &format!("dev snapshot read failed: {e}"),
                );
                0
            }
        }
    }

    /// Age + row caps on every file, then the whole-cache byte cap.
    pub fn prune(&self, now: i64) {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.prune_locked(now);
    }

    fn prune_locked(&self, now: i64) {
        let mut files: Vec<(String, String, Vec<Row>)> = Vec::new();
        for flavor in self.flavors() {
            for key in self.characters(&flavor) {
                let mut rows = self.rows(&flavor, &key);
                let before = rows.len();
                prune_rows(&mut rows, now);
                if rows.len() != before {
                    self.write(&flavor, &key, &rows);
                }
                files.push((flavor.clone(), key, rows));
            }
        }
        // Each row's on-disk size, computed once (oldest first, like the rows).
        let mut sizes: Vec<VecDeque<u64>> = files
            .iter()
            .map(|(_, _, rows)| rows.iter().map(row_bytes).collect())
            .collect();
        let mut file_bytes: Vec<u64> = sizes.iter().map(|s| s.iter().sum()).collect();
        let mut total: u64 = file_bytes.iter().sum();
        let mut dropped = vec![0usize; files.len()];
        while total > MAX_CACHE_BYTES {
            let Some(i) = (0..files.len())
                .filter(|&i| sizes[i].len() > 1)
                .max_by_key(|&i| file_bytes[i])
            else {
                break;
            };
            let b = sizes[i].pop_front().unwrap_or(0);
            file_bytes[i] -= b;
            total = total.saturating_sub(b);
            dropped[i] += 1;
        }
        for (i, (_, _, rows)) in files.iter_mut().enumerate() {
            rows.drain(..dropped[i]);
        }
        let dirty: Vec<bool> = dropped.iter().map(|&n| n > 0).collect();
        for (i, (flavor, key, rows)) in files.iter().enumerate() {
            if dirty[i] {
                self.write(flavor, key, rows);
            }
        }
    }
}

/// D10 dedupe. Same digest as the newest row → bump it (a re-read of the
/// same capture is a no-op); else append. Returns whether `rows` changed.
pub fn merge(rows: &mut Vec<Row>, snapshot: Value, now: i64) -> bool {
    let d = digest(&snapshot);
    let captured = snapshot["capturedAt"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if let Some(last) = rows.last_mut().filter(|r| r.digest == d) {
        if secs(&captured) <= secs(&last.last_captured_at) {
            return false;
        }
        last.last_captured_at = captured;
        last.seen_count += 1;
        return true;
    }
    if rows
        .iter()
        .any(|r| r.digest == d && r.last_captured_at == captured)
    {
        return false;
    }
    let ms = rows
        .last()
        .and_then(|r| id_ms(&r.snapshot_id))
        .map_or(now_ms(now), |last| now_ms(now).max(last + 1));
    rows.push(Row {
        snapshot_id: new_snapshot_id(ms),
        received_at: rfc3339_utc_from_unix_seconds(now),
        captured_at: captured.clone(),
        last_captured_at: captured,
        seen_count: 1,
        digest: d,
        snapshot,
    });
    true
}

/// Drop rows not seen for 7 days (never the newest), then keep the newest 200.
pub fn prune_rows(rows: &mut Vec<Row>, now: i64) {
    let cutoff = now - RETAIN_SECS;
    let last = rows.len().saturating_sub(1);
    let mut i = 0;
    rows.retain(|r| {
        let keep = i == last || secs(&r.last_captured_at) >= cutoff;
        i += 1;
        keep
    });
    let extra = rows.len().saturating_sub(MAX_ROWS_PER_FILE);
    rows.drain(..extra);
}
