//! ROK-2068 fix 3: the BG3 per-campaign upload picker.

use crate::bg3_campaigns::{
    adopt_defaults, campaign_name, dtos, first_sight, group, select, still_ticked_by, ticked,
    valid_key, CampaignGroup, CampaignStore, SaveCampaign, DEFAULT_ON_SECS,
};
use crate::collector::PendingUpload;
use crate::collector_view::campaigns_for;
use crate::config::AppConfig;
use crate::manifest::{BG3_SAVE, WOW_LEDGERLINK};
use serde_json::json;
use std::cell::RefCell;
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
        campaign: None,
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

#[test]
fn unticking_mid_batch_stops_that_campaigns_remaining_uploads() {
    let tagged = |label: &str, key: Option<&str>| PendingUpload {
        campaign: key.map(str::to_owned),
        ..upload(label)
    };
    let batch = [
        tagged("A1", Some("k-a")),
        tagged("B1", Some("k-b")),
        tagged("WoW", None),
        tagged("B2", Some("k-b")),
    ];
    let live = RefCell::new(BTreeMap::new());
    let mut sent = Vec::new();
    for p in still_ticked_by(&batch, || live.borrow().clone()) {
        sent.push(p.label.clone());
        if p.label == "A1" {
            live.borrow_mut().insert("k-b".to_owned(), false);
        }
    }
    assert_eq!(
        sent,
        ["A1", "WoW"],
        "k-b was unticked after A1 was sent: no k-b save may upload"
    );
}

#[test]
fn first_sight_persists_the_30_day_default_on_or_off() {
    let groups = [grp("k-new", 1), grp("k-old", 40)];
    let fresh = first_sight(&groups, &BTreeMap::new(), NOW);
    let want = BTreeMap::from([("k-new".to_owned(), true), ("k-old".to_owned(), false)]);
    assert_eq!(fresh, want, "a never-seen campaign gets its 30-day default");
    let mut ticks = BTreeMap::new();
    assert!(adopt_defaults(&mut ticks, fresh.clone()), "new keys: write");
    assert_eq!(ticks, want);
    assert!(
        !adopt_defaults(&mut ticks, fresh),
        "nothing new: the config is not written again"
    );
}

#[test]
fn a_new_save_in_a_stored_off_campaign_stays_off() {
    let ticks = BTreeMap::from([("k-old".to_owned(), false)]);
    let found = vec![(save("k-old", "Tav", 0), Some(upload("fresh save")))];
    let (groups, pending) = select(found, &ticks, NOW);
    assert!(pending.is_empty(), "stored OFF beats the 30-day rule");
    assert!(first_sight(&groups, &ticks, NOW).is_empty(), "already seen");
}

#[test]
fn a_user_tick_wins_over_a_first_sight_default() {
    let mut ticks = BTreeMap::from([("k-old".to_owned(), true)]);
    let groups = [grp("k-old", 40)];
    assert!(first_sight(&groups, &ticks, NOW).is_empty());
    let late = BTreeMap::from([("k-old".to_owned(), false)]);
    assert!(
        !adopt_defaults(&mut ticks, late),
        "a pick is never replaced"
    );
    assert_eq!(ticks.get("k-old"), Some(&true));
}

#[test]
fn a_store_hands_out_first_sight_once_and_keeps_groups() {
    let store = CampaignStore::new();
    let fresh = BTreeMap::from([("k-a".to_owned(), true)]);
    store.remember(vec![grp("k-a", 1)], fresh.clone());
    assert_eq!(store.take_first_sight(), fresh);
    assert!(store.take_first_sight().is_empty(), "taken once");
    assert_eq!(store.groups(), [grp("k-a", 1)]);
}

#[test]
fn only_the_bg3_row_carries_campaigns() {
    let cfg = AppConfig::default();
    let groups = [grp("k-a", 1)];
    let rows = campaigns_for(BG3_SAVE.id, &cfg, &groups, NOW);
    assert_eq!(
        rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        ["k-a"]
    );
    assert!(campaigns_for(WOW_LEDGERLINK.id, &cfg, &groups, NOW).is_empty());
}

#[test]
fn a_campaign_is_named_by_its_newest_save_even_out_of_order() {
    let groups = group(&[save("k-a", "Older", 5), save("k-a", "Newest", 1)]);
    assert_eq!(groups[0].name, "Newest");
}
