//! ROK-2067 Lane A1: `/config` parsing, fetch, cache and refetch debounce.
//!
//! `fixtures/server_config.json` is hand-written from Raid Ledger 2d669aceb
//! (`CompanionConfigSchema` in `packages/contract/src/companion-ingest-api.schema.ts`,
//! `packages/contract/companion/envelope/v1/CONTRACT.md` §4.2). Raid Ledger
//! ships no `/config` fixture to pin, so it lives here, not under `contracts/`.

use super::TOKEN;
use crate::config::AppConfig;
use crate::server_config::{
    config_url, fetch, FetchError, PluginState, ServerConfig, ServerState, DEFER_DEBOUNCE,
};
use crate::token::DeviceToken;
use serde_json::json;
use std::time::{Duration, Instant};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const FIXTURE: &str = include_str!("fixtures/server_config.json");

fn parse(v: serde_json::Value) -> ServerConfig {
    serde_json::from_value(v).expect("config parses")
}

#[test]
fn the_fixture_parses_plugin_and_game() {
    let sc: ServerConfig = serde_json::from_str(FIXTURE).expect("fixture parses");
    let bg3 = sc.contract("bg3-save").expect("bg3 listed");
    assert!(!bg3.enabled);
    let plugin = bg3.plugin.as_ref().expect("plugin");
    assert_eq!(
        (plugin.name.as_str(), plugin.state),
        ("Baldur's Gate 3", PluginState::NotInstalled)
    );
    let game = bg3.game.as_ref().expect("game");
    assert_eq!(game.slug, "baldurs-gate-iii");
    assert!(game
        .cover_url
        .as_deref()
        .is_some_and(|u| u.starts_with("//images.igdb.com/")));
    assert!(sc.contract("wow-ledgerlink").is_some_and(|c| c.enabled));
}

#[test]
fn unknown_keys_are_ignored_at_every_level() {
    let sc = parse(json!({
        "configVersion": 2, "brandNew": {"x": 1},
        "contracts": [{
            "id": "bg3-save", "enabled": true, "futureKey": [1],
            "plugin": {"id": "bg3", "name": "BG3", "state": "active", "extra": true},
            "game": {"slug": "b", "name": "B", "coverUrl": null, "art": {}}
        }]
    }));
    let c = sc.contract("bg3-save").expect("listed");
    assert!(c.enabled && c.plugin.is_some() && c.game.is_some());
}

#[test]
fn an_old_server_without_plugin_or_game_still_parses() {
    let sc = parse(json!({ "contracts": [{ "id": "bg3-save", "enabled": false }] }));
    let c = sc.contract("bg3-save").expect("listed");
    assert_eq!((c.plugin.as_ref(), c.game.as_ref()), (None, None));
    let null = parse(
        json!({ "contracts": [{ "id": "x", "enabled": true, "plugin": null, "game": null }] }),
    );
    assert!(null.contract("x").is_some_and(|c| c.plugin.is_none()));
    let anon = parse(json!({ "configVersion": 1, "minAppVersion": "0.1.0" }));
    assert_eq!(anon.contracts, None, "anonymous answer has no contracts");
}

#[test]
fn a_plugin_state_from_a_newer_server_is_unknown_not_an_error() {
    let sc = parse(json!({ "contracts": [{ "id": "x", "enabled": true,
        "plugin": {"id": "p", "name": "P", "state": "quarantined"} }] }));
    let state = sc
        .contract("x")
        .and_then(|c| c.plugin.as_ref())
        .map(|p| p.state);
    assert_eq!(state, Some(PluginState::Unknown));
}

#[test]
fn config_url_sits_under_the_api_prefix() {
    let u = config_url("https://raid.gamernight.net").expect("url");
    assert_eq!(
        u.as_str(),
        "https://raid.gamernight.net/api/companion/v1/config"
    );
}

#[tokio::test]
async fn fetch_sends_the_device_token_and_parses_the_answer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/companion/v1/config"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(FIXTURE),
        )
        .mount(&server)
        .await;
    let token = DeviceToken::parse(TOKEN).expect("token");
    let sc = fetch(&server.uri(), &token).await.expect("fetched");
    assert_eq!(sc.contracts.as_ref().map(Vec::len), Some(2));
}

#[tokio::test]
async fn fetch_reports_a_non_2xx_as_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let token = DeviceToken::parse(TOKEN).expect("token");
    let err = fetch(&server.uri(), &token).await.expect_err("401 fails");
    assert_eq!(err, FetchError::Unauthorized, "a 401 means re-link");
    assert!(err.to_string().contains("401"), "{err}");
}

#[tokio::test]
async fn fetch_reports_a_5xx_as_a_plain_failure_not_a_revocation() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let token = DeviceToken::parse(TOKEN).expect("token");
    let err = fetch(&server.uri(), &token).await.expect_err("503 fails");
    assert_eq!(
        err,
        FetchError::Other("the server answered HTTP 503".into())
    );
}

#[tokio::test]
async fn a_server_change_forgets_the_old_covers_and_wakes_the_config_task() {
    let dir = super::temp_dir("server-changed");
    let rt = crate::runtime::Runtime::new(dir.join("sent.json"));
    rt.update_server(|s| {
        s.reachable = Some(true);
        s.covers.insert("bg3-save".into(), Some("data:old".into()));
    });
    rt.server_changed();
    let s = rt.server();
    assert_eq!(
        (s.reachable, s.covers.len()),
        (None, 0),
        "old server's state kept"
    );
    let woke = tokio::time::timeout(Duration::from_millis(50), rt.config_kick.notified()).await;
    assert!(woke.is_ok(), "the /config task was not woken");
}

#[test]
fn a_config_401_shows_the_unlinked_tiles_not_the_cache() {
    let dir = super::temp_dir("config-401");
    let rt = crate::runtime::Runtime::new(dir.join("sent.json"));
    let cfg = AppConfig {
        last_server_config: Some(serde_json::from_str(FIXTURE).expect("fixture")),
        ..AppConfig::default()
    };
    let line = |rt: &crate::runtime::Runtime| {
        let s = rt.snapshot(&cfg);
        let t = s
            .tiles
            .iter()
            .find(|t| t.id == "bg3-save")
            .map(|t| t.line.clone());
        (s.relink, t.unwrap_or_default())
    };
    assert!(
        !line(&rt).1.starts_with("Link this device"),
        "linked: cached state"
    );
    rt.config_unauthorized();
    let (relink, text) = line(&rt);
    assert!(relink, "the window is asked to re-link");
    assert!(text.starts_with("Link this device"), "tile line: {text}");
}

#[test]
fn deferral_refetch_is_debounced_to_once_a_minute() {
    let mut s = ServerState::default();
    let t0 = Instant::now();
    assert!(s.take_defer_slot(t0), "the first deferral refetches");
    assert!(
        !s.take_defer_slot(t0 + Duration::from_secs(59)),
        "within 60 s: no"
    );
    assert!(s.take_defer_slot(t0 + DEFER_DEBOUNCE), "after 60 s: yes");
}

#[test]
fn the_cache_round_trips_and_an_old_config_file_still_loads() {
    let dir = super::temp_dir("server-config-cache");
    let file = dir.join("config.json");
    std::fs::write(
        &file,
        r#"{"serverUrl":"https://slot-1.gamernight.net","wowDir":"D:\\WoW"}"#,
    )
    .expect("write old file");
    let old = AppConfig::load(&file);
    assert_eq!(
        old.server_url, "https://slot-1.gamernight.net",
        "old file kept its URL"
    );
    assert_eq!(
        (old.last_server_config.as_ref(), old.fetched_at.as_ref()),
        (None, None)
    );
    let cfg = AppConfig {
        last_server_config: Some(serde_json::from_str(FIXTURE).expect("fixture")),
        fetched_at: Some("2026-11-02T10:00:00Z".into()),
        ..old
    };
    cfg.save(&file).expect("save");
    assert_eq!(AppConfig::load(&file), cfg);
}

/// A `Shared` on `origin` with an in-memory token store.
fn shared_on(name: &str, origin: &str) -> crate::state::Shared {
    let dir = super::temp_dir(name);
    let shared = crate::state::Shared::new(
        dir.join("config.json"),
        Box::new(crate::token::MemoryStore::default()),
    );
    shared
        .update_config(|c| c.server_url = origin.to_owned())
        .expect("save config");
    shared
}

#[test]
fn a_relink_between_a_config_request_and_its_401_keeps_the_new_token() {
    let origin = "https://raid.example";
    let shared = shared_on("config-401-relink", origin);
    let rt = crate::runtime::Runtime::new(super::temp_dir("config-401-relink-rt").join("s.json"));
    let old = DeviceToken::parse(TOKEN).expect("token");
    let new = DeviceToken::parse(&format!("rlcd_{}", "N".repeat(43))).expect("token");
    shared.save_token(origin, &old).expect("save");
    // The request went out with `old`; the user re-linked before its 401.
    shared.save_token(origin, &new).expect("re-link");
    let unlinked = crate::server_config::revoke_after_401(&shared, &rt, origin, &old);
    assert_eq!(
        (
            unlinked,
            shared.token().expect("load").map(|t| t.hint().to_owned())
        ),
        (false, Some(new.hint().to_owned())),
        "a late 401 for the old token deleted the re-linked one"
    );
    assert!(
        !rt.snapshot(&shared.config()).relink,
        "re-link asked after a good link"
    );
    // The 401 for the token still stored does revoke it.
    assert!(crate::server_config::revoke_after_401(
        &shared, &rt, origin, &new
    ));
    assert!(
        shared.token().expect("load").is_none(),
        "the refused token is kept"
    );
}

#[test]
fn a_server_switch_between_a_config_request_and_its_401_keeps_the_token() {
    let shared = shared_on("config-401-switch", "https://a.example");
    let rt = crate::runtime::Runtime::new(super::temp_dir("config-401-switch-rt").join("s.json"));
    let t = DeviceToken::parse(TOKEN).expect("token");
    shared.save_token("https://a.example", &t).expect("save");
    shared
        .update_config(|c| c.server_url = "https://b.example".into())
        .expect("switch");
    assert!(!crate::server_config::revoke_after_401(
        &shared,
        &rt,
        "https://a.example",
        &t
    ));
    shared
        .update_config(|c| c.server_url = "https://a.example".into())
        .expect("switch back");
    assert!(
        shared.token().expect("load").is_some(),
        "the old server's token was cleared"
    );
}

fn one_cover(url: &str) -> std::collections::BTreeMap<String, Option<String>> {
    [("bg3-save".to_owned(), Some(url.to_owned()))].into()
}

#[test]
fn a_refresh_for_the_old_server_does_not_overwrite_the_reset_state() {
    let rt = crate::runtime::Runtime::new(super::temp_dir("stale-refresh").join("s.json"));
    let old = rt.update_server(ServerState::begin_refresh);
    rt.server_changed();
    let stored = rt.update_server(|s| s.publish(old, Some(true), one_cover("data:old")));
    let s = rt.server();
    assert_eq!(
        (stored, s.reachable, s.covers.len()),
        (false, None, 0),
        "the old server's covers came back after the switch"
    );
}

#[test]
fn an_older_concurrent_refresh_does_not_overwrite_a_newer_one() {
    let rt = crate::runtime::Runtime::new(super::temp_dir("older-refresh").join("s.json"));
    let older = rt.update_server(ServerState::begin_refresh);
    let newer = rt.update_server(ServerState::begin_refresh);
    assert!(rt.update_server(|s| s.publish(newer, Some(true), one_cover("data:new"))));
    let stored = rt.update_server(|s| s.publish(older, Some(false), one_cover("data:old")));
    let s = rt.server();
    assert_eq!(
        (
            stored,
            s.reachable,
            s.covers.get("bg3-save").cloned().flatten()
        ),
        (false, Some(true), Some("data:new".to_owned())),
        "the older refresh overwrote the newer one"
    );
}
