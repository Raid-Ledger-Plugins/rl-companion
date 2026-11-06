//! ROK-2067 D6/D7/D9/D12: game tiles, the plugin-off copy swap and
//! per-collector results (spec ACs 5, 6, 8, 9, 11a, 14).

use crate::collector::PendingUpload;
use crate::collector_status::{Line, LineStatus};
use crate::collector_view::rows;
use crate::config::AppConfig;
use crate::manifest::{Platform, BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::{effect, Runtime};
use crate::tiles::{last_checked, tiles, TileDto, TileInputs, TileState};
use crate::upload::{UploadOutcome, NOT_TAKEN_YET};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const AT: &str = "2026-11-02T10:00:00Z";

fn env(_: &str) -> Option<String> {
    None
}

fn with(contracts: Value) -> AppConfig {
    AppConfig {
        last_server_config: Some(
            serde_json::from_value(json!({ "contracts": contracts })).expect("config"),
        ),
        fetched_at: Some(AT.into()),
        ..AppConfig::default()
    }
}

fn plugin(state: &str) -> Value {
    json!({ "id": "bg3", "name": "Baldur's Gate 3", "state": state })
}

fn view(cfg: &AppConfig, linked: bool, covers: &BTreeMap<String, Option<String>>) -> Vec<TileDto> {
    let r = rows(
        cfg,
        Some(Platform::Macos),
        &env,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    tiles(&TileInputs {
        cfg,
        linked,
        rows: &r,
        covers,
    })
}

fn by_id(cfg: &AppConfig, id: &str) -> TileDto {
    view(cfg, true, &BTreeMap::new())
        .into_iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("no tile {id}"))
}

#[test]
fn supported_when_the_contract_is_enabled() {
    let cfg = with(json!([{ "id": "bg3-save", "enabled": true, "plugin": plugin("active") }]));
    let t = by_id(&cfg, BG3_SAVE.contract_id);
    assert_eq!(
        (t.state, t.chip.as_deref(), t.banner.as_deref()),
        (TileState::Supported, None, None)
    );
    assert_eq!(t.line, "Off", "the collector's own status line");
    assert!(t.clickable);
}

#[test]
fn plugin_off_names_the_plugin_with_install_or_turn_on_copy() {
    let t = by_id(
        &with(json!([{ "id": "bg3-save", "enabled": false, "plugin": plugin("not_installed") }])),
        "bg3-save",
    );
    assert_eq!(t.state, TileState::PluginOff);
    assert_eq!(
        (t.line.as_str(), t.chip.as_deref()),
        ("Plugin not installed", Some("Plugin not installed"))
    );
    assert_eq!(t.banner.as_deref(), Some("Uploads are paused. This server doesn't have the Baldur's Gate 3 plugin installed. Ask your Raid Ledger admin to install it."));
    assert_eq!(t.plugin_name.as_deref(), Some("Baldur's Gate 3"));
    assert!(t.clickable);
    let off = by_id(
        &with(json!([{ "id": "bg3-save", "enabled": false, "plugin": plugin("inactive") }])),
        "bg3-save",
    );
    assert_eq!(
        (off.state, off.line.as_str()),
        (TileState::PluginOff, "Plugin turned off")
    );
    assert!(off.banner.as_deref().is_some_and(
        |b| b.contains("plugin turned off. Ask your Raid Ledger admin to turn it on.")
    ));
}

#[test]
fn not_on_server_when_unlisted_or_disabled_with_the_plugin_on() {
    for contracts in [
        json!([]),
        json!([{ "id": "bg3-save", "enabled": false, "plugin": plugin("active") }]),
        json!([{ "id": "bg3-save", "enabled": false, "plugin": null }]),
    ] {
        let t = by_id(&with(contracts.clone()), "bg3-save");
        assert_eq!(t.state, TileState::NotOnServer, "{contracts}");
        assert_eq!(t.line, "Not supported on this server");
        assert_eq!(t.chip.as_deref(), Some("Not on this server"));
        assert_eq!(
            t.banner.as_deref(),
            Some("This server doesn't take Baldur's Gate 3 uploads. Your settings are kept.")
        );
        assert!(t.clickable, "settings stay usable");
    }
}

#[test]
fn an_old_server_without_plugin_lets_enabled_decide() {
    let cfg = with(
        json!([{ "id": "bg3-save", "enabled": true }, { "id": "wow-ledgerlink", "enabled": false }]),
    );
    assert_eq!(by_id(&cfg, "bg3-save").state, TileState::Supported);
    assert_eq!(by_id(&cfg, "wow-ledgerlink").state, TileState::NotOnServer);
}

#[test]
fn server_only_contracts_follow_the_app_ones_a_to_z_and_are_not_clickable() {
    let cfg = with(json!([
        { "id": "zeta-log", "enabled": true, "plugin": { "id": "z", "name": "Zeta", "state": "active" } },
        { "id": "alpha-log", "enabled": true, "plugin": null },
        { "id": "bg3-save", "enabled": true }
    ]));
    let all = view(&cfg, true, &BTreeMap::new());
    let ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        ["wow-ledgerlink", "bg3-save", "alpha-log", "zeta-log"],
        "D6 order, one tile per game (ROK-2073)"
    );
    let zeta = &all[3];
    assert_eq!(
        (zeta.state, zeta.clickable, zeta.collector_id.as_deref()),
        (TileState::AppTooOld, false, None)
    );
    assert_eq!(
        (zeta.title.as_str(), zeta.line.as_str()),
        ("Zeta", "Update the Companion to use this")
    );
    assert_eq!(all[2].title, "alpha-log", "no plugin: the id");
    assert!(
        zeta.lines.is_empty(),
        "a server-only tile has no collector lines"
    );
}

#[test]
fn unlinked_and_unfetched_tiles_are_neutral() {
    let cfg = with(
        json!([{ "id": "bg3-save", "enabled": false, "plugin": plugin("inactive") }, { "id": "new-one", "enabled": true }]),
    );
    let unlinked = view(&cfg, false, &BTreeMap::new());
    assert_eq!(unlinked.len(), 2, "no server-only tiles while unlinked");
    assert!(unlinked
        .iter()
        .all(|t| t.state == TileState::Unknown && t.chip.is_none()));
    assert_eq!(
        unlinked[1].line,
        "Link this device to see what raid.gamernight.net supports"
    );
    let never = view(&AppConfig::default(), true, &BTreeMap::new());
    assert!(never
        .iter()
        .all(|t| t.state == TileState::Unknown && t.line == "Checking the server…"));
}

#[test]
fn last_checked_shows_only_when_unreachable_with_a_cache() {
    let cfg = with(json!([]));
    assert_eq!(last_checked(&cfg, true, Some(false)).as_deref(), Some(AT));
    assert_eq!(last_checked(&cfg, true, Some(true)), None);
    assert_eq!(last_checked(&cfg, false, Some(false)), None);
    assert_eq!(last_checked(&AppConfig::default(), true, Some(false)), None);
}

#[test]
fn titles_prefer_the_server_game_then_the_manifest() {
    let cfg = with(json!([{ "id": "wow-ledgerlink", "enabled": true,
        "game": { "slug": "world-of-warcraft-forever", "name": "World of Warcraft: Forever", "coverUrl": null } }]));
    assert_eq!(
        by_id(&cfg, "wow-ledgerlink").title,
        "World of Warcraft: Forever"
    );
    let plain = by_id(&AppConfig::default(), "wow-ledgerlink");
    assert_eq!(
        (plain.title.as_str(), plain.subtitle.as_deref()),
        ("World of Warcraft", Some("LedgerLink addon"))
    );
    assert_eq!(
        by_id(&AppConfig::default(), "bg3-save").title,
        "Baldur's Gate 3"
    );
}

#[test]
fn a_cover_is_passed_through_and_none_sets_the_fallback() {
    let covers = BTreeMap::from([(
        WOW_LEDGERLINK.contract_id.to_owned(),
        Some("data:image/jpeg;base64,AA==".to_owned()),
    )]);
    let t = view(&AppConfig::default(), true, &covers);
    assert_eq!(
        (t[0].cover.as_deref(), t[0].fallback),
        (Some("data:image/jpeg;base64,AA=="), false)
    );
    assert_eq!(
        (t[1].cover.as_deref(), t[1].fallback, t[1].initials.as_str()),
        (None, true, "BG")
    );
}

fn upload(contract: &'static str, label: &str) -> PendingUpload {
    PendingUpload {
        campaign: None,
        contract,
        scope: "https://rl.test|/shared".into(),
        item: "save".into(),
        sha256: "aa".into(),
        label: label.into(),
        body: json!({}),
        blobs: vec![],
    }
}

#[test]
fn ac11_results_sharing_scope_and_item_stay_apart_per_collector() {
    let rt = Runtime::new(super::temp_dir("ac11-results").join("sent.json"));
    let deferred = UploadOutcome::Deferred {
        reason: NOT_TAKEN_YET.into(),
        status: None,
        code: None,
    };
    rt.finish(&upload("bg3-save", "AutoSave_9"), &effect(&deferred));
    rt.finish(
        &upload("wow-ledgerlink", "Thrall"),
        &effect(&UploadOutcome::Accepted),
    );
    let results = rt.snapshot(&AppConfig::default()).results;
    let tagged: Vec<(&str, &str, bool)> = results
        .iter()
        .map(|r| (r.collector_id.as_str(), r.label.as_str(), r.ok))
        .collect();
    assert_eq!(tagged.len(), 2, "one result per collector: {tagged:?}");
    assert!(
        tagged.contains(&("bg3-save", "AutoSave_9", false)),
        "{tagged:?}"
    );
    assert!(
        tagged.contains(&("wow-ledgerlink", "Thrall", true)),
        "{tagged:?}"
    );
}

#[test]
fn d9_plugin_off_replaces_the_generic_not_taken_copy() {
    let rt = Runtime::new(super::temp_dir("d9-copy").join("sent.json"));
    let fx = effect(&UploadOutcome::Deferred {
        reason: NOT_TAKEN_YET.into(),
        status: None,
        code: None,
    });
    rt.finish(&upload("bg3-save", "AutoSave_9"), &fx);
    let line = format!("Uploaded 0 of 1 export(s); {}", fx.text);
    rt.set_lines(
        BTreeMap::from([
            (
                "bg3-save".to_owned(),
                Line::new(LineStatus::Retrying, line.clone()),
            ),
            (
                "wow-ledgerlink".to_owned(),
                Line::new(LineStatus::Retrying, line),
            ),
        ]),
        false,
    );
    let cfg = with(json!([
        { "id": "bg3-save", "enabled": false, "plugin": plugin("not_installed") },
        { "id": "wow-ledgerlink", "enabled": true }
    ]));
    let s = rt.snapshot(&cfg);
    let bg3 = s
        .collectors
        .iter()
        .find(|r| r.id == "bg3-save")
        .expect("bg3 row");
    assert_eq!(bg3.status, "Plugin not installed");
    assert!(
        s.results.iter().all(|r| !r.result.contains(NOT_TAKEN_YET)),
        "{:?}",
        s.results
    );
    let wow = s
        .collectors
        .iter()
        .find(|r| r.id == "wow-ledgerlink")
        .expect("wow row");
    assert!(
        wow.status.contains(NOT_TAKEN_YET),
        "a supported game keeps its own line"
    );
}

#[test]
fn the_status_serialises_tiles_and_collector_ids_in_camel_case() {
    let rt = Runtime::new(super::temp_dir("tiles-json").join("sent.json"));
    rt.log("Rate limited");
    rt.log_for("bg3-save", "AutoSave_9: Uploaded");
    rt.finish(
        &upload("bg3-save", "AutoSave_9"),
        &effect(&UploadOutcome::Accepted),
    );
    let v = serde_json::to_value(rt.snapshot(&AppConfig::default())).expect("json");
    assert_eq!(v["tiles"][0]["collectorId"], "wow-ledgerlink");
    assert_eq!(v["tiles"][0]["state"], "unknown");
    assert_eq!(v["results"][0]["collectorId"], "bg3-save");
    assert_eq!(v["log"][0]["collectorId"], "bg3-save");
    assert_eq!(v["log"][1]["collectorId"], Value::Null, "device-level line");
    assert_eq!(v["lastChecked"], Value::Null);
}
