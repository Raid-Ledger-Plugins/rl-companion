//! ROK-2073: one tile per game. BG3 saves + screenshots share one tile with a
//! line per collector; the tile's chip is the most urgent line's.

use crate::collector_view::rows;
use crate::config::AppConfig;
use crate::manifest::Platform;
use crate::tiles::{headline, tiles, TileDto, TileInputs, TileLineDto, TileState};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn with(contracts: Value) -> AppConfig {
    AppConfig {
        last_server_config: Some(
            serde_json::from_value(json!({ "contracts": contracts })).expect("config"),
        ),
        ..AppConfig::default()
    }
}

fn plugin(state: &str) -> Value {
    json!({ "id": "bg3", "name": "Baldur's Gate 3", "state": state })
}

fn all(cfg: &AppConfig) -> Vec<TileDto> {
    let r = rows(
        cfg,
        Some(Platform::Macos),
        &|_| None,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    tiles(&TileInputs {
        cfg,
        linked: true,
        rows: &r,
        covers: &BTreeMap::new(),
    })
}

fn bg3(cfg: &AppConfig) -> TileDto {
    all(cfg)
        .into_iter()
        .find(|t| t.title == "Baldur's Gate 3")
        .expect("bg3 tile")
}

fn line_states(t: &TileDto) -> Vec<(&str, &str, TileState)> {
    t.lines
        .iter()
        .map(|l| (l.collector_id.as_str(), l.label.as_str(), l.state))
        .collect()
}

#[test]
fn ac1_bg3_saves_and_screenshots_are_one_tile_with_two_lines() {
    let cfg = with(json!([
        { "id": "bg3-save", "enabled": true },
        { "id": "bg3-screenshot", "enabled": true }
    ]));
    let tiles = all(&cfg);
    let titles: Vec<&str> = tiles.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(
        titles,
        ["World of Warcraft", "Baldur's Gate 3"],
        "one tile per game"
    );
    let t = bg3(&cfg);
    assert_eq!(
        line_states(&t),
        [
            ("bg3-save", "Saves", TileState::Supported),
            ("bg3-screenshot", "Screenshots", TileState::Supported)
        ]
    );
    assert_eq!(
        (t.id.as_str(), t.collector_id.as_deref()),
        ("bg3-save", Some("bg3-save"))
    );
    assert_eq!(
        t.subtitle, None,
        "no per-collector subtitle on a shared tile"
    );
}

#[test]
fn wow_stays_one_tile_with_its_single_line() {
    let wow = all(&with(json!([{ "id": "wow-ledgerlink", "enabled": true }])))
        .into_iter()
        .next()
        .expect("wow");
    assert_eq!(wow.subtitle.as_deref(), Some("LedgerLink addon"));
    assert_eq!(
        line_states(&wow),
        [("wow-ledgerlink", "LedgerLink addon", TileState::Supported)]
    );
}

#[test]
fn ac2_a_partly_supported_game_keeps_not_on_server_on_its_line() {
    let t = bg3(&with(json!([{ "id": "bg3-save", "enabled": true }])));
    assert_eq!(t.state, TileState::Supported, "the tile is not greyed");
    assert_eq!(t.chip, None);
    let shots = t.lines.get(1).expect("screenshot line");
    assert_eq!(
        (shots.state, shots.chip.as_deref(), shots.line.as_str()),
        (
            TileState::NotOnServer,
            Some("Not on this server"),
            "Not supported on this server"
        )
    );
    assert_eq!(
        shots.banner.as_deref(),
        Some("This server doesn't take Baldur's Gate 3 screenshots. Your settings are kept.")
    );
    assert_eq!(t.lines.first().map(|l| l.state), Some(TileState::Supported));
}

#[test]
fn a_game_with_no_collector_on_the_server_is_greyed_with_a_game_banner() {
    let t = bg3(&with(json!([])));
    assert_eq!(
        (t.state, t.chip.as_deref()),
        (TileState::NotOnServer, Some("Not on this server"))
    );
    assert_eq!(
        t.banner.as_deref(),
        Some("This server doesn't take Baldur's Gate 3 uploads. Your settings are kept.")
    );
}

#[test]
fn the_chip_is_the_most_urgent_line_plugin_off_over_supported() {
    let t = bg3(&with(json!([
        { "id": "bg3-save", "enabled": true },
        { "id": "bg3-screenshot", "enabled": false, "plugin": plugin("not_installed") }
    ])));
    assert_eq!(
        (t.state, t.chip.as_deref(), t.line.as_str()),
        (
            TileState::PluginOff,
            Some("Plugin not installed"),
            "Plugin not installed"
        )
    );
    assert_eq!(t.plugin_name.as_deref(), Some("Baldur's Gate 3"));
    assert_eq!(
        t.lines.first().map(|l| l.state),
        Some(TileState::Supported),
        "saves keep theirs"
    );
}

fn line(state: TileState, id: &str) -> TileLineDto {
    TileLineDto {
        collector_id: id.into(),
        label: id.into(),
        state,
        chip: None,
        line: String::new(),
        banner: None,
        plugin_name: None,
    }
}

#[test]
fn headline_precedence_plugin_off_then_unknown_then_supported_ties_go_first() {
    use TileState::*;
    let pick = |ls: &[TileLineDto]| headline(ls).map(|l| l.collector_id.clone());
    let cases: [(&[(TileState, &str)], &str); 6] = [
        (&[(Supported, "a"), (PluginOff, "b")], "b"),
        (&[(Unknown, "a"), (Supported, "b")], "a"),
        (&[(Supported, "a"), (Supported, "b")], "a"),
        (&[(Supported, "a"), (NotOnServer, "b")], "a"),
        (&[(NotOnServer, "a"), (PluginOff, "b")], "b"),
        (&[(NotOnServer, "a"), (NotOnServer, "b")], "a"),
    ];
    for (input, want) in cases {
        let ls: Vec<TileLineDto> = input.iter().map(|(s, id)| line(*s, id)).collect();
        assert_eq!(pick(&ls).as_deref(), Some(want), "{input:?}");
    }
    assert_eq!(headline(&[]), None);
}

#[test]
fn minor1_grouping_keys_on_game_key_not_the_display_name() {
    use crate::manifest::{CollectorManifest, BG3_SAVE, BG3_SCREENSHOT};
    use crate::tiles::group_by_game;
    let reworded = CollectorManifest {
        game_name: "Baldur's Gate III",
        ..BG3_SCREENSHOT
    };
    let other = CollectorManifest {
        id: "other",
        game_key: "other-game",
        ..BG3_SAVE
    };
    let ids = |ms: &[&CollectorManifest]| -> Vec<Vec<&str>> {
        group_by_game(ms.iter().copied())
            .iter()
            .map(|g| g.iter().map(|m| m.id).collect())
            .collect()
    };
    assert_eq!(
        ids(&[&BG3_SAVE, &reworded]),
        [vec!["bg3-save", "bg3-screenshot"]],
        "a reworded name keeps one tile"
    );
    assert_eq!(
        ids(&[&BG3_SAVE, &other]),
        [vec!["bg3-save"], vec!["other"]],
        "the same name with another key is another game"
    );
    assert_eq!(BG3_SAVE.game_key, BG3_SCREENSHOT.game_key);
}

#[test]
fn minor2_the_title_comes_from_any_collector_the_server_names_a_game_for() {
    let cfg = with(json!([{ "id": "bg3-screenshot", "enabled": true,
        "game": { "slug": "baldurs-gate-iii", "name": "Baldur's Gate III", "coverUrl": null } }]));
    let t = all(&cfg)
        .into_iter()
        .find(|t| t.id == "bg3-save")
        .expect("bg3 tile");
    assert_eq!(
        t.title, "Baldur's Gate III",
        "screenshots' server title, though saves are not on the server"
    );
}
