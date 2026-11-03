//! ROK-2068 fix 3: the BG3 per-campaign upload picker.

use crate::bg3_campaigns::{
    campaign_name, dtos, group, select, ticked, valid_key, CampaignGroup, SaveCampaign,
    DEFAULT_ON_SECS,
};
use crate::collector::PendingUpload;
use crate::config::AppConfig;
use serde_json::json;
use std::collections::BTreeMap;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 24 * 60 * 60;

fn save(key: &str, name: &str, age_days: i64) -> SaveCampaign {
    SaveCampaign {
        key: key.to_owned(),
        name: name.to_owned(),
        saved_unix: NOW - age_days * DAY,
    }
}

fn upload(label: &str) -> PendingUpload {
    PendingUpload {
        contract: "bg3-save",
        scope: format!("o|{label}"),
        item: "save".into(),
        sha256: "0".repeat(64),
        label: label.into(),
        body: json!({}),
        blobs: Vec::new(),
    }
}

fn grp(key: &str, age_days: i64) -> CampaignGroup {
    CampaignGroup {
        key: key.into(),
        name: key.into(),
        newest_unix: NOW - age_days * DAY,
        count: 1,
    }
}

#[test]
fn saves_group_by_key_named_by_their_newest_save_newest_campaign_first() {
    let saves = [
        save("k-a", "Tav", 1),
        save("k-b", "Karlach run", 3),
        save("k-a", "Tav (older)", 5),
        save("k-a", "Tav (oldest)", 9),
    ];
    let groups = group(&saves);
    let got: Vec<(&str, &str, i64, usize)> = groups
        .iter()
        .map(|g| (g.key.as_str(), g.name.as_str(), g.newest_unix, g.count))
        .collect();
    assert_eq!(
        got,
        [
            ("k-a", "Tav", NOW - DAY, 3),
            ("k-b", "Karlach run", NOW - 3 * DAY, 1)
        ]
    );
}

#[test]
fn a_never_seen_campaign_is_on_only_within_30_days() {
    let none = BTreeMap::new();
    assert!(ticked(&grp("k", 0), &none, NOW), "today → on");
    assert!(ticked(&grp("k", 29), &none, NOW), "29 days → on");
    assert!(!ticked(&grp("k", 31), &none, NOW), "31 days → off");
    assert!(!ticked(&grp("k", 800), &none, NOW), "the 800-day run → off");
    assert_eq!(DEFAULT_ON_SECS, 30 * DAY);
}

#[test]
fn a_persisted_tick_overrides_the_default_both_ways() {
    let ticks: BTreeMap<String, bool> =
        [("old".to_owned(), true), ("new".to_owned(), false)].into();
    assert!(
        ticked(&grp("old", 800), &ticks, NOW),
        "user ticked an old run"
    );
    assert!(
        !ticked(&grp("new", 1), &ticks, NOW),
        "user unticked a new run"
    );
}

#[test]
fn only_ticked_campaigns_upload_and_newest_first_order_is_kept() {
    let found = vec![
        (save("on", "A", 1), Some(upload("A1"))),
        (save("off", "B", 2), Some(upload("B1"))),
        (save("on", "A", 3), None),
        (save("on", "A", 4), Some(upload("A2"))),
        (save("off", "B", 5), Some(upload("B2"))),
        (save("stale", "C", 400), Some(upload("C1"))),
    ];
    let ticks: BTreeMap<String, bool> = [("off".to_owned(), false)].into();
    let (groups, pending) = select(found, &ticks, NOW);
    let labels: Vec<&str> = pending.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(
        labels,
        ["A1", "A2"],
        "unticked and stale campaigns are held back"
    );
    let counts: Vec<(&str, usize)> = groups.iter().map(|g| (g.key.as_str(), g.count)).collect();
    assert_eq!(
        counts,
        [("on", 3), ("off", 2), ("stale", 1)],
        "every save is counted"
    );
}

#[test]
fn rows_carry_name_newest_date_count_and_both_tick_states() {
    let rows = dtos(&[grp("k", 800)], &BTreeMap::new(), NOW);
    let v = serde_json::to_value(&rows).expect("json");
    assert_eq!(
        v,
        json!([{
            "key": "k", "name": "k",
            "newestSavedAt": rl_collectors::rfc3339_utc_from_unix_seconds(NOW - 800 * DAY),
            "saveCount": 1, "ticked": false, "defaultOn": false,
        }])
    );
}

#[test]
fn the_name_is_the_party_leader_else_the_save_name() {
    assert_eq!(
        campaign_name(Some("Shadowheart"), "QuickSave_3"),
        "Shadowheart"
    );
    assert_eq!(campaign_name(Some("  "), "QuickSave_3"), "QuickSave_3");
    assert_eq!(campaign_name(Some("a\u{7}b"), "QuickSave_3"), "QuickSave_3");
    assert_eq!(campaign_name(None, "QuickSave_3"), "QuickSave_3");
}

#[test]
fn a_config_from_before_the_picker_still_loads() {
    let dir = super::temp_dir("bg3-old-cfg");
    let file = dir.join("config.json");
    let old = json!({ "serverUrl": "https://raid.gamernight.net", "wowDir": "D:/WoW",
        "collectorsEnabled": { "bg3-save": true }, "autostartInitialized": true });
    std::fs::write(&file, old.to_string()).expect("write");
    let cfg = AppConfig::load(&file);
    assert!(cfg.bg3_campaigns.is_empty());
    assert_eq!(
        cfg.wow_dir, "D:/WoW",
        "the rest of the file is kept, not defaulted"
    );
    assert!(cfg.collector_enabled("bg3-save", false));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_sha256_keys_are_accepted_from_the_window() {
    assert!(valid_key(&"a1".repeat(32)));
    assert!(!valid_key("short"));
    assert!(!valid_key(&"A1".repeat(32)));
}
