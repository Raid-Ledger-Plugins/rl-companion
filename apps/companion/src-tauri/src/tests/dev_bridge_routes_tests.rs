//! Dev bridge read endpoints (ROK-2055 C2-L2): every body validates against
//! the pinned `dev-bridge/v1` schema and stays ≤ 16 KiB with `next` paging
//! (AC-6); `/logs` is redacted (AC-7); the WoW-folder refresh hook.

use super::dev_bridge_unit_tests::schema_errors;
use crate::config::AppConfig;
use crate::dev_bridge::auth::AuthPolicy;
use crate::dev_bridge::logbuf::{redact, LogBuffer};
use crate::dev_bridge::paging::PAGE_MAX_BYTES;
use crate::dev_bridge::server::{ServerCtx, Snapshot};
use crate::dev_bridge::token::sha256_hex;
use crate::dev_bridge::{on_wow_dir_changed, DevBridge};
use hyper::Method;
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::Arc;

const MULTI: &[u8] =
    include_bytes!("../../../../../crates/rl-collectors/tests/fixtures/wow_sv/multi-entry.lua");
pub(super) const CALLER: Ipv4Addr = Ipv4Addr::new(192, 168, 0, 229);
pub(super) const FLAVOR: &str = "_anniversary_";

fn token() -> String {
    format!("rlcdev_{}", "A".repeat(43))
}

fn snap(wow: &Path) -> Snapshot {
    Snapshot {
        policy: AuthPolicy {
            allowlist: vec![CALLER],
            token_sha256: Some(sha256_hex(&token())),
        },
        wow_dir: wow.to_string_lossy().into_owned(),
        probes_enabled: false,
        snapshot_cache: None,
    }
}

pub(super) fn ctx(wow: &Path) -> ServerCtx {
    let ctx = ServerCtx::with_logs(Box::leak(Box::new(LogBuffer::default())));
    ctx.set_snapshot(snap(wow));
    ctx
}

pub(super) fn get(ctx: &ServerCtx, path: &str, query: Option<&str>) -> (u16, Value) {
    let auth = format!("Bearer {}", token());
    ctx.route(IpAddr::V4(CALLER), &Method::GET, path, query, Some(&auth))
}

/// Every page: 200, valid against `def`, ≤ 16 KiB. Returns all pages.
pub(super) fn pages(ctx: &ServerCtx, path: &str, def: &str, base: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        let q = match &cursor {
            Some(c) => format!("{base}&cursor={c}"),
            None => base.to_owned(),
        };
        let (status, body) = get(ctx, path, Some(&q));
        assert_eq!(status, 200, "{path}?{q} → {body}");
        assert_eq!(
            schema_errors(def, &body),
            Vec::<String>::new(),
            "{path} page valid"
        );
        let len = serde_json::to_vec(&body).expect("json").len();
        assert!(len <= PAGE_MAX_BYTES, "{path} page is {len} B, over 16 KiB");
        cursor = body["next"].as_str().map(str::to_owned);
        out.push(body);
        if cursor.is_none() {
            return out;
        }
    }
    panic!("{path}: paging never ended");
}

pub(super) fn write(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, bytes).expect("write");
}

#[test]
fn addons_page_through_every_folder_once() {
    let wow = super::temp_dir("bridge-addons");
    for i in 0..300 {
        let dir = wow
            .join(FLAVOR)
            .join("Interface/AddOns")
            .join(format!("Addon{i:03}"));
        let toc = format!(
            "## Interface: 11507\n## Title: {} \"quoted\" €\n## Version: 1.{i}\n## SavedVariables: DbA{i}, DbB{i}\n## LoadOnDemand: {}\n",
            "Long title ".repeat(20),
            i % 2
        );
        write(&dir.join(format!("Addon{i:03}.toc")), toc.as_bytes());
    }
    let c = ctx(&wow);
    let all = pages(&c, "/dev/v1/addons", "addons", &format!("flavor={FLAVOR}"));
    assert!(all.len() > 1, "300 long rows need more than one page");
    let folders: Vec<String> = all
        .iter()
        .flat_map(|p| p["addons"].as_array().cloned().unwrap_or_default())
        .map(|a| a["folder"].as_str().unwrap_or("").to_owned())
        .collect();
    let want: Vec<String> = (0..300).map(|i| format!("Addon{i:03}")).collect();
    assert_eq!(folders, want, "every folder exactly once, in order");
    let first = &all[0]["addons"][1];
    assert_eq!(first["savedVariables"], json!(["DbA1", "DbB1"]));
    assert_eq!(first["loadOnDemand"], json!(true));
}

#[test]
fn sv_raw_pages_reassemble_the_file() {
    let wow = super::temp_dir("bridge-sv-raw");
    let mut text = String::from("LedgerLinkDB = {\n");
    for i in 0..3000 {
        text.push_str(&format!("\t[\"k{i}\"] = \"é\\\"x\\n€\u{1}\",\n"));
    }
    text.push_str("}\n");
    let rel = format!("{FLAVOR}/WTF/Account/TESTACCOUNT/SavedVariables/LedgerLink.lua");
    write(&wow.join(&rel), text.as_bytes());
    let c = ctx(&wow);
    let all = pages(
        &c,
        "/dev/v1/ledgerlink/sv",
        "sv-raw",
        &format!("flavor={FLAVOR}&scope=account"),
    );
    assert!(all.len() > 3, "a {} B file needs several pages", text.len());
    let joined: String = all
        .iter()
        .map(|p| p["text"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(joined, text, "pages concatenate to the file");
    assert_eq!(
        all[0]["path"],
        json!(rel),
        "path is relative to the WoW folder"
    );
    let (status, _) = get(
        &c,
        "/dev/v1/ledgerlink/sv",
        Some(&format!("flavor={FLAVOR}&scope=character")),
    );
    assert_eq!(status, 404, "no character file → 404");
}

#[test]
fn sv_parsed_hashes_guids_and_validates() {
    let wow = super::temp_dir("bridge-sv-parsed");
    write(
        &wow.join(format!(
            "{FLAVOR}/WTF/Account/TESTACCOUNT/SavedVariables/LedgerLink.lua"
        )),
        MULTI,
    );
    write(
        &wow.join(format!(
            "{FLAVOR}/WTF/Account/TESTACCOUNT/Testrealm/Testchar/SavedVariables/LedgerLink.lua"
        )),
        b"LedgerLinkDB = {\n\t[\"settings\"] = {},\n}\n",
    );
    let c = ctx(&wow);
    let all = pages(
        &c,
        "/dev/v1/ledgerlink/sv/parsed",
        "sv-parsed",
        &format!("flavor={FLAVOR}"),
    );
    let files = all[0]["files"].as_array().cloned().unwrap_or_default();
    assert_eq!(files.len(), 2);
    assert!(
        !files[0]["outbox"].as_array().is_none_or(Vec::is_empty),
        "multi-entry outbox parsed"
    );
    assert_eq!(files[1]["character"], json!("Testchar"));
    assert_eq!(files[1]["topKeys"], json!(["settings"]));
    let body = serde_json::to_string(&all).expect("json");
    assert!(
        !body.contains("Player-"),
        "GUIDs never leave the bridge un-hashed"
    );
}

#[test]
fn flavor_and_cursor_are_checked() {
    let wow = super::temp_dir("bridge-flavor");
    std::fs::create_dir_all(wow.join(FLAVOR)).expect("mkdir");
    std::fs::create_dir_all(wow.join("_classic_era_")).expect("mkdir");
    let c = ctx(&wow);
    for q in [
        "flavor=..",
        "flavor=_retail_",
        "",
        &format!("flavor={FLAVOR}&cursor=%21%21"),
    ] {
        let (status, body) = get(&c, "/dev/v1/addons", Some(q));
        assert_eq!(status, 400, "?{q}");
        assert_eq!(schema_errors("error", &body), Vec::<String>::new());
    }
    let (status, body) = c.route(IpAddr::V4(CALLER), &Method::GET, "/dev/v1/logs", None, None);
    assert_eq!(
        (status, body),
        (401, json!({ "error": "unauthorized" })),
        "no bearer → 401 before routing"
    );
    let (status, _) = c.route(
        IpAddr::V4(Ipv4Addr::new(192, 168, 0, 9)),
        &Method::GET,
        "/dev/v1/collectors",
        None,
        Some(&format!("Bearer {}", token())),
    );
    assert_eq!(status, 403, "caller outside the allowlist → 403");
}

#[test]
fn logs_are_redacted_paged_and_filtered() {
    let buf: &'static LogBuffer = Box::leak(Box::new(LogBuffer::default()));
    for i in 0..600 {
        let level = if i % 3 == 0 { "warn" } else { "info" };
        let msg = format!(
            "#{i} Authorization: Bearer rlcdev_{i}abcDEF-_ device RLCD_xyz{i} BEARER tok{i} secret={i}pw from C:\\Users\\Jake Doe\\x and /Users/jake/wow {}",
            "pad ".repeat(30)
        );
        buf.push(
            level,
            "dev_bridge",
            &msg,
            format!("2026-11-01T18:{:02}:00Z", i % 60),
        );
    }
    let c = ServerCtx::with_logs(buf);
    c.set_snapshot(snap(Path::new("")));
    let all = pages(&c, "/dev/v1/logs", "logs", "level=info");
    let lines: Vec<Value> = all
        .iter()
        .flat_map(|p| p["lines"].as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(lines.len(), 600, "every line once across pages");
    assert!(
        lines[599]["msg"]
            .as_str()
            .unwrap_or("")
            .starts_with("#599 "),
        "oldest first"
    );
    let text = serde_json::to_string(&lines).expect("json").to_lowercase();
    for leak in [
        "rlcdev_",
        "rlcd_",
        "bearer rlcd",
        "bearer tok",
        "secret=0pw",
        "jake",
    ] {
        assert!(!text.contains(leak), "logs leak {leak:?}");
    }
    let warn = pages(&c, "/dev/v1/logs", "logs", "level=warn");
    assert_eq!(
        warn.iter()
            .map(|p| p["lines"].as_array().map_or(0, Vec::len))
            .sum::<usize>(),
        200
    );
}

#[test]
fn redact_applies_the_contract_patterns() {
    assert_eq!(redact("missing bearer token"), "missing bearer <redacted>");
    assert_eq!(
        redact("Bearer   rlcdev_AbC-_9 done"),
        "Bearer <redacted> done"
    );
    assert_eq!(
        redact("dev RLCDEV_x and rlcd_ and rlcd_abc"),
        "dev <redacted> and <redacted> and <redacted>"
    );
    assert_eq!(redact("api_key: abc123, ok"), "api_key: <redacted>, ok");
    assert_eq!(
        redact("/home/alice/x C:\\Users\\Bob\\y"),
        "/home/<user>/x C:\\Users\\<user>\\y"
    );
    assert_eq!(
        redact("GET /dev/v1/health 200 412B"),
        "GET /dev/v1/health 200 412B"
    );
}

#[test]
fn collectors_serves_the_runtime_source() {
    let c = ctx(Path::new(""));
    let (status, body) = get(&c, "/dev/v1/collectors", None);
    assert_eq!(
        (status, body),
        (200, json!({ "collectors": [] })),
        "empty before the runtime starts"
    );
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../contracts/dev-bridge/v1/fixtures/collectors.json"
    ))
    .expect("fixture");
    let served = fixture.clone();
    c.set_collectors(Arc::new(move || served.clone()));
    let (status, body) = get(&c, "/dev/v1/collectors", None);
    assert_eq!(status, 200);
    assert_eq!(body, fixture);
    assert_eq!(schema_errors("collectors", &body), Vec::<String>::new());
}

#[test]
fn wow_folder_change_refreshes_the_served_snapshot() {
    let wow = super::temp_dir("bridge-refresh");
    std::fs::create_dir_all(wow.join(FLAVOR)).expect("mkdir");
    let bridge = DevBridge::new(wow.join("tls.json"), None);
    let mut cfg = AppConfig::default();
    cfg.dev_bridge.allowlist = vec![CALLER.to_string()];
    cfg.dev_bridge.token_sha256 = Some(sha256_hex(&token()));
    cfg.wow_dir = "/nowhere".into();
    bridge.refresh(&cfg);
    cfg.wow_dir = wow.to_string_lossy().into_owned();
    on_wow_dir_changed(None, &cfg);
    let (_, before) = get(bridge.ctx(), "/dev/v1/health", None);
    assert_eq!(before["flavors"], json!([]), "None bridge is a no-op");
    on_wow_dir_changed(Some(&bridge), &cfg);
    let (_, after) = get(bridge.ctx(), "/dev/v1/health", None);
    assert_eq!(
        after["flavors"],
        json!([FLAVOR]),
        "the new folder is served at once"
    );
    assert_eq!(after["wowDir"], json!(cfg.wow_dir.replace('\\', "/")));
}

/// A worst-case LedgerLink.lua: 40 outbox entries with 128-char section and
/// source, and 32 top-level keys of 64 chars.
fn worst_case_sv() -> String {
    let real = rl_collectors::parse_ledgerlink_outbox(MULTI).expect("fixture parses");
    let text = &real.entries.first().expect("an entry").text;
    let mut lua = String::from("LedgerLinkDB = {\n");
    for k in 0..32 {
        lua.push_str(&format!("\t[\"{}{k:02}\"] = 1,\n", "k".repeat(62)));
    }
    lua.push_str("\t[\"outbox\"] = {\n");
    for i in 0..40 {
        lua.push_str(&format!(
            "\t\t[\"Player-0000-{i:08X}\"] = {{\n\t\t\t[\"at\"] = 1790000000,\n\t\t\t[\"exportedAt\"] = 1790000000,\n\t\t\t[\"section\"] = \"{}\",\n\t\t\t[\"source\"] = \"{}\",\n\t\t\t[\"text\"] = \"{text}\",\n\t\t}},\n",
            "s".repeat(128),
            "o".repeat(128),
        ));
    }
    lua.push_str("\t},\n}\n");
    lua
}

#[test]
fn sv_parsed_worst_case_row_splits_under_16_kib() {
    let wow = super::temp_dir("bridge-sv-worst");
    let (acct, realm, ch) = ("A".repeat(100), "R".repeat(100), "C".repeat(100));
    let rel = format!("{FLAVOR}/WTF/Account/{acct}/{realm}/{ch}/SavedVariables/LedgerLink.lua");
    write(&wow.join(rel), worst_case_sv().as_bytes());
    let c = ctx(&wow);
    // `pages` asserts every page is schema-valid and ≤ 16 KiB.
    let all = pages(
        &c,
        "/dev/v1/ledgerlink/sv/parsed",
        "sv-parsed",
        &format!("flavor={FLAVOR}"),
    );
    let rows: Vec<Value> = all
        .iter()
        .flat_map(|p| p["files"].as_array().cloned().unwrap_or_default())
        .collect();
    assert!(
        rows.len() > 1,
        "the 40-entry outbox is split across rows, got {}",
        rows.len()
    );
    for r in &rows {
        let len = serde_json::to_vec(r).expect("json").len();
        assert!(
            len <= crate::dev_bridge::sv::ROW_MAX_BYTES,
            "row is {len} B"
        );
        assert_eq!(r["character"], json!(ch), "every split row names the file");
    }
    let entries: usize = rows
        .iter()
        .map(|r| r["outbox"].as_array().map_or(0, Vec::len))
        .sum();
    assert_eq!(entries, 40, "no outbox entry is lost by the split");
    assert_eq!(rows[0]["topKeys"].as_array().map_or(0, Vec::len), 32);
}

#[test]
fn sv_raw_cursor_refuses_a_file_rewritten_mid_paging() {
    let wow = super::temp_dir("bridge-sv-rewrite");
    let path = wow.join(format!(
        "{FLAVOR}/WTF/Account/TESTACCOUNT/SavedVariables/LedgerLink.lua"
    ));
    write(&path, "x".repeat(40_000).as_bytes());
    let c = ctx(&wow);
    let base = format!("flavor={FLAVOR}&scope=account");
    let (status, first) = get(&c, "/dev/v1/ledgerlink/sv", Some(&base));
    assert_eq!(status, 200);
    let next = first["next"].as_str().expect("a second page").to_owned();
    write(&path, "y".repeat(40_001).as_bytes());
    let (status, body) = get(
        &c,
        "/dev/v1/ledgerlink/sv",
        Some(&format!("{base}&cursor={next}")),
    );
    assert_eq!(
        status, 400,
        "a stale cursor is refused, not spliced: {body}"
    );
    assert_eq!(schema_errors("error", &body), Vec::<String>::new());
    assert_eq!(body["issues"][0]["path"], json!(["cursor"]));
}

#[cfg(unix)]
#[test]
fn sv_symlinked_ledgerlink_lua_is_not_followed() {
    let wow = super::temp_dir("bridge-sv-symlink");
    let outside = wow.join("outside.lua");
    write(&outside, b"LedgerLinkDB = {\n\t[\"secret\"] = 1,\n}\n");
    let link = wow.join(format!(
        "{FLAVOR}/WTF/Account/TESTACCOUNT/SavedVariables/LedgerLink.lua"
    ));
    std::fs::create_dir_all(link.parent().expect("parent")).expect("mkdir");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");
    let c = ctx(&wow);
    let (status, body) = get(
        &c,
        "/dev/v1/ledgerlink/sv",
        Some(&format!("flavor={FLAVOR}&scope=account")),
    );
    assert_eq!(
        status, 404,
        "a symlinked LedgerLink.lua is not read: {body}"
    );
    let (_, parsed) = get(
        &c,
        "/dev/v1/ledgerlink/sv/parsed",
        Some(&format!("flavor={FLAVOR}")),
    );
    assert_eq!(parsed["files"], json!([]), "nor listed in sv/parsed");
}

#[test]
fn every_refusal_body_matches_the_error_def() {
    let wow = super::temp_dir("bridge-error-bodies");
    std::fs::create_dir_all(wow.join(FLAVOR)).expect("mkdir");
    let c = ctx(&wow);
    let mut seen = Vec::new();
    let mut check = |status: u16, body: Value| {
        assert_eq!(
            schema_errors("error", &body),
            Vec::<String>::new(),
            "{status} body {body}"
        );
        seen.push(status);
    };
    let (s, b) = get(&c, "/dev/v1/nope", None);
    check(s, b);
    let (s, b) = get(
        &c,
        "/dev/v1/ledgerlink/sv",
        Some(&format!("flavor={FLAVOR}&scope=account")),
    );
    check(s, b);
    let (s, b) = c.route(IpAddr::V4(CALLER), &Method::GET, "/dev/v1/logs", None, None);
    check(s, b);
    let outsider = IpAddr::V4(Ipv4Addr::new(192, 168, 0, 9));
    let auth = format!("Bearer {}", token());
    let (s, b) = c.route(outsider, &Method::GET, "/dev/v1/logs", None, Some(&auth));
    check(s, b);
    let wrong = format!("Bearer rlcdev_{}", "B".repeat(43));
    let mut last = (0, Value::Null);
    for _ in 0..12 {
        last = c.route(
            IpAddr::V4(CALLER),
            &Method::GET,
            "/dev/v1/logs",
            None,
            Some(&wrong),
        );
    }
    check(last.0, last.1);
    assert_eq!(seen, vec![404, 404, 401, 403, 429]);
}

#[test]
fn redact_user_names_with_spaces_and_debug_escapes() {
    let cases = [
        (r"from C:\Users\Jake Doe\x", r"from C:\Users\<user>\x"),
        (r"path C:\\Users\\Jake\\wow", r"path C:\\Users\\<user>\\wow"),
        (r"C:\\Users\\Jake Doe\\wow", r"C:\\Users\\<user>\\wow"),
        ("/Users/jake/wow and more", "/Users/<user>/wow and more"),
        ("at /home/jake done", "at /home/<user> done"),
    ];
    for (raw, want) in cases {
        assert_eq!(redact(raw), want, "redact({raw:?})");
    }
}

#[test]
fn since_is_parsed_as_rfc3339_utc_and_orders_fractions() {
    use crate::dev_bridge::logbuf::parse_utc;
    for bad in [
        "Z",
        "2026-11-01Z",
        "2026-13-01T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-11-01T00:00:00",
    ] {
        assert_eq!(parse_utc(bad), None, "{bad:?} is not RFC 3339 UTC");
    }
    let whole = parse_utc("2026-11-01T18:00:00Z").expect("whole");
    let frac = parse_utc("2026-11-01T18:00:00.5Z").expect("fraction");
    assert!(
        frac > whole,
        "…:00.5Z sorts after …:00Z (a text compare puts it first)"
    );
    assert_eq!(parse_utc("1970-01-01T00:00:01Z"), Some((1, 0)));
    let buf: &'static LogBuffer = Box::leak(Box::new(LogBuffer::default()));
    buf.push("info", "dev_bridge", "early", "2026-11-01T18:00:00Z".into());
    buf.push(
        "info",
        "dev_bridge",
        "late",
        "2026-11-01T18:00:00.5Z".into(),
    );
    let c = ServerCtx::with_logs(buf);
    c.set_snapshot(snap(Path::new("")));
    let (status, _) = get(&c, "/dev/v1/logs", Some("since=Z"));
    assert_eq!(status, 400, "since=Z is refused");
    let (_, body) = get(&c, "/dev/v1/logs", Some("since=2026-11-01T18:00:00.1Z"));
    let msgs: Vec<&str> = body["lines"].as_array().map_or(Vec::new(), |l| {
        l.iter().filter_map(|x| x["msg"].as_str()).collect()
    });
    assert_eq!(
        msgs,
        vec!["late"],
        "only the line at/after the fractional since"
    );
}
