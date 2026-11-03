//! ROK-2061 lane 3: per-game rows, device state, folder override and the
//! first-upload preview hold.

use crate::collector::{Blob, PendingUpload};
use crate::collector_view::{apply_root, device_note, device_state, rows, RETRYING};
use crate::config::AppConfig;
use crate::manifest::{Platform, BG3_SAVE, BG3_SCREENSHOT, WOW_LEDGERLINK};
use crate::preview::{base64, hold};
use crate::runtime::{
    effect, secs_left, shown_text, stopped_line, take_expired, tick_scans, Runtime,
};
use crate::upload::UploadOutcome;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

fn env(k: &str) -> Option<String> {
    (k == "HOME").then(|| "/home/ada".to_owned())
}

fn pending(name: &str, saved_at: &str, sha: &str) -> PendingUpload {
    PendingUpload {
        campaign: None,
        contract: "bg3-save",
        scope: format!("https://rl.test|/saves/{name}.lsv"),
        item: "save".into(),
        sha256: sha.into(),
        label: name.into(),
        body: json!({ "payload": {
            "saveName": name, "savedAt": saved_at, "campaignKey": "abcdef0123456789",
            "currentLevel": "WLD_Main_A", "difficulty": "Balanced",
            "party": [{ "origin": "Astarion", "race": "High Elf", "level": 5,
                        "classes": [{ "main": "Rogue", "sub": "Thief" }, { "main": "Wizard", "sub": null }] }]
        }}),
        blobs: vec![Blob {
            sha256: "0".repeat(64),
            content_type: "image/webp".into(),
            bytes: b"RIFF".to_vec(),
        }],
    }
}

#[test]
fn device_state_is_one_line_in_priority_order() {
    assert_eq!(device_state(false, None, Ok(true)), None);
    assert_eq!(
        device_state(false, None, Ok(false)).as_deref(),
        Some("Link this device to upload")
    );
    assert!(device_state(true, Some(9), Ok(false)).is_some_and(|s| s.starts_with("Paused")));
    assert_eq!(
        device_state(false, Some(9), Ok(false)).as_deref(),
        Some("Rate limited — retrying in 9s")
    );
    assert!(device_state(false, None, Err("locked".into())).is_some_and(|s| s.contains("locked")));
}

#[test]
fn one_row_per_registered_collector_with_detected_folder() {
    let cfg = AppConfig::default();
    let r = rows(
        &cfg,
        Some(Platform::Macos),
        &env,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    let ids: Vec<&str> = r.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, [WOW_LEDGERLINK.id, BG3_SAVE.id, BG3_SCREENSHOT.id]);
    let bg3 = &r[1];
    assert!(!bg3.enabled && bg3.confirm_first_upload && !bg3.consented);
    assert_eq!(bg3.status, "Off");
    assert!(bg3
        .detected_folder
        .as_deref()
        .is_some_and(|d| d.starts_with("/home/ada/Documents/Larian")));
    assert_eq!(bg3.override_note, None);
}

#[test]
fn a_stored_relative_override_shows_the_rejection_note() {
    let mut cfg = AppConfig::default();
    cfg.collector_roots
        .insert(BG3_SAVE.id.into(), vec!["saves/story".into()]);
    let r = rows(
        &cfg,
        Some(Platform::Macos),
        &env,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert_eq!(r[1].folder_override, "saves/story");
    let note = r[1].override_note.clone().unwrap_or_default();
    assert!(note.contains("must be a full path"), "note was {note:?}");
}

#[test]
fn apply_root_saves_absolute_clears_blank_and_refuses_the_rest() {
    let mut cfg = AppConfig::default();
    let err = apply_root(&mut cfg, BG3_SAVE.id, "relative/dir", &env).unwrap_err();
    assert!(err.contains("must be a full path"), "{err}");
    assert!(
        cfg.collector_roots.is_empty(),
        "a refused folder is not saved"
    );
    // `/`-rooted paths are not absolute on Windows: build them from a real
    // absolute folder so the test means the same on every OS (Codex P2).
    let story = std::env::temp_dir().join("games").join("bg3").join("Story");
    let drive = story
        .ancestors()
        .last()
        .expect("an absolute path has a root");
    let story = story.to_string_lossy().into_owned();
    assert!(
        apply_root(&mut cfg, BG3_SAVE.id, &drive.to_string_lossy(), &env)
            .unwrap_err()
            .contains("whole drive")
    );
    apply_root(&mut cfg, BG3_SAVE.id, &format!(" {story} "), &env).expect("absolute is accepted");
    assert_eq!(cfg.roots_override(BG3_SAVE.id), Some(vec![story.clone()]));
    let climbing = format!("{story}{}..", std::path::MAIN_SEPARATOR);
    assert!(apply_root(&mut cfg, BG3_SAVE.id, &climbing, &env)
        .unwrap_err()
        .contains("must not use"));
    assert_eq!(
        cfg.roots_override(BG3_SAVE.id),
        Some(vec![story]),
        "a refused override leaves the saved one alone"
    );
    apply_root(&mut cfg, BG3_SAVE.id, "  ", &env).expect("blank resets");
    assert_eq!(cfg.roots_override(BG3_SAVE.id), None);
    assert!(apply_root(&mut cfg, WOW_LEDGERLINK.id, "/x/y", &env).is_err());
    assert!(apply_root(&mut cfg, "nope", "/x/y", &env)
        .unwrap_err()
        .contains("Unknown collector"));
}

#[test]
fn preview_summarises_the_newest_save_and_keeps_every_key() {
    let batch = [
        pending("Old", "2026-10-01T00:00:00Z", "a1"),
        pending("New", "2026-11-01T00:00:00Z", "b2"),
    ];
    let held = hold(BG3_SAVE.id, &batch).expect("non-empty batch is held");
    let p = &held.preview;
    assert_eq!(
        (p.save_name.as_str(), p.campaign.as_str(), p.waiting),
        ("New", "abcdef01", 2)
    );
    assert_eq!(p.location, "WLD_Main_A");
    assert_eq!(p.party[0].name, "Astarion");
    assert_eq!(
        p.party[0].summary,
        "Level 5 High Elf Rogue (Thief) / Wizard"
    );
    assert_eq!(
        p.thumbnail.as_deref(),
        Some("data:image/webp;base64,UklGRg==")
    );
    assert_eq!(held.keys.len(), 2);
    assert!(hold(BG3_SAVE.id, &[]).is_none());
}

fn shot(taken_at: &str, sha: &str, webp: &[u8]) -> PendingUpload {
    PendingUpload {
        campaign: None,
        contract: "bg3-screenshot",
        scope: format!("https://rl.test|/shots/{sha}.jpg"),
        item: "screenshot".into(),
        sha256: sha.into(),
        label: format!("Screenshot {sha}"),
        body: json!({ "contract": "bg3-screenshot", "payload": {
            "takenAt": taken_at, "width": 1920, "height": 1080,
            "target": { "hubGuildId": 42 }
        }}),
        blobs: vec![Blob {
            sha256: "1".repeat(64),
            content_type: "image/webp".into(),
            bytes: webp.to_vec(),
        }],
    }
}

#[test]
fn screenshot_preview_shows_the_newest_shot_its_hub_and_no_save_fields() {
    // `files()` lists newest first; the old `savedAt` key picked the LAST (oldest) one.
    let batch = [
        shot("2026-11-03T12:00:00Z", "new", b"NEW"),
        shot("2026-11-01T08:00:00Z", "old", b"OLD"),
    ];
    let held = hold(BG3_SCREENSHOT.id, &batch).expect("non-empty batch is held");
    let p = &held.preview;
    assert_eq!(p.kind, "screenshot");
    assert_eq!(p.taken_at, "2026-11-03T12:00:00Z", "newest by takenAt");
    assert_eq!(
        p.thumbnail.as_deref(),
        Some(format!("data:image/webp;base64,{}", base64(b"NEW")).as_str()),
        "the newest shot's encoded WebP is the image shown"
    );
    assert_eq!(p.hub_guild_id, Some(42));
    assert_eq!((p.waiting, held.keys.len()), (2, 2));
    assert!(p.save_name.is_empty() && p.party.is_empty());
    let save = hold(BG3_SAVE.id, &[pending("A", "2026-11-01T00:00:00Z", "a")]).expect("held");
    assert_eq!(
        (save.preview.kind.as_str(), save.preview.hub_guild_id),
        ("save", None)
    );
}

#[test]
fn base64_matches_rfc4648_vectors() {
    let cases = [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
    ];
    for (raw, want) in cases {
        assert_eq!(base64(raw.as_bytes()), want, "input {raw:?}");
    }
}

#[test]
fn skip_records_every_held_save_and_clears_the_preview() {
    let dir = super::temp_dir("skip-held");
    let rt = Runtime::new(dir.join("sent.json"));
    let batch = [
        pending("A", "2026-10-01T00:00:00Z", "a1"),
        pending("B", "2026-10-02T00:00:00Z", "b2"),
    ];
    rt.set_held(BG3_SAVE.id, hold(BG3_SAVE.id, &batch));
    let cfg = AppConfig::default();
    let before = rt.snapshot(&cfg);
    assert!(before.collectors[1]
        .preview
        .as_ref()
        .is_some_and(|p| p.save_name == "B"));
    assert_eq!(rt.skip_held(BG3_SAVE.id), 2);
    let sent = rt.sent();
    assert_eq!(
        sent.scope(&batch[0].scope).get("save").map(String::as_str),
        Some("a1")
    );
    assert_eq!(
        sent.scope(&batch[1].scope).get("save").map(String::as_str),
        Some("b2")
    );
    assert_eq!(rt.snapshot(&cfg).collectors[1].preview, None);
    assert_eq!(rt.skip_held(BG3_SAVE.id), 0, "nothing left to skip");
}

#[test]
fn consent_round_trips_and_is_omitted_when_empty() {
    let mut cfg = AppConfig::default();
    assert!(!serde_json::to_string(&cfg)
        .unwrap_or_default()
        .contains("collectorConsent"));
    cfg.set_consented(BG3_SAVE.id);
    let json = serde_json::to_string(&cfg).unwrap_or_default();
    assert!(
        json.contains(r#""collectorConsent":{"bg3-save":true}"#),
        "{json}"
    );
    let back: AppConfig = serde_json::from_str(&json).unwrap_or_default();
    assert!(back.consented(BG3_SAVE.id) && !back.consented(WOW_LEDGERLINK.id));
}

#[test]
fn every_save_keeps_its_own_result_when_one_is_rejected_and_another_lands() {
    let dir = super::temp_dir("two-save-results");
    let rt = Runtime::new(dir.join("sent.json"));
    let rejected = pending("A", "2026-10-01T00:00:00Z", "a1");
    let accepted = pending("B", "2026-10-02T00:00:00Z", "b2");
    rt.finish(
        &rejected,
        &effect(&UploadOutcome::Rejected {
            status: 422,
            code: "invalid".into(),
            issue: None,
        }),
    );
    rt.finish(&accepted, &effect(&UploadOutcome::Accepted));
    let results = rt.snapshot(&AppConfig::default()).results;
    let by_label: Vec<(&str, bool)> = results.iter().map(|r| (r.label.as_str(), r.ok)).collect();
    assert_eq!(by_label.len(), 2, "both saves keep a result: {by_label:?}");
    assert!(by_label.contains(&("A", false)), "{by_label:?}");
    assert!(by_label.contains(&("B", true)), "{by_label:?}");
}

#[test]
fn a_401_sets_the_structured_relink_flag_whatever_the_joined_line_says() {
    let dir = super::temp_dir("relink-flag");
    let rt = Runtime::new(dir.join("sent.json"));
    let cfg = AppConfig::default();
    let fx = effect(&UploadOutcome::Unauthorized);
    assert!(fx.relink, "a 401 clears the token");
    let lines = BTreeMap::from([
        (WOW_LEDGERLINK.id.to_owned(), "Up to date".to_owned()),
        (BG3_SAVE.id.to_owned(), "Re-link this device".to_owned()),
    ]);
    rt.set_lines(lines.clone(), fx.relink);
    rt.set_status("Up to date · Re-link this device");
    let s = rt.snapshot(&cfg);
    assert_ne!(s.status, "Re-link this device", "the line is joined");
    assert!(s.relink, "the window learns of the 401 from the flag");
    rt.set_device(false, Ok(false));
    rt.set_lines(lines, false);
    assert!(!rt.snapshot(&cfg).relink, "the next scan clears it");
}

fn relinked_runtime(tag: &str) -> Runtime {
    let rt = Runtime::new(super::temp_dir(tag).join("sent.json"));
    let fx = effect(&UploadOutcome::Unauthorized);
    let lines = BTreeMap::from([(WOW_LEDGERLINK.id.to_owned(), fx.text.clone())]);
    rt.set_lines(lines, fx.relink);
    assert!(rt.snapshot(&AppConfig::default()).relink, "the 401 set it");
    rt
}

#[test]
fn a_successful_link_after_a_401_clears_the_relink_flag() {
    let rt = relinked_runtime("relink-then-link");
    rt.linked();
    let s = rt.snapshot(&AppConfig::default());
    assert!(!s.relink, "a fresh link is not a re-link notice");
}

#[test]
fn a_mid_scan_update_after_a_re_link_does_not_carry_the_old_401() {
    let rt = relinked_runtime("relink-mid-scan");
    rt.linked();
    rt.begin_scan();
    rt.set_status("Uploading 2 export(s)…");
    assert!(!rt.snapshot(&AppConfig::default()).relink, "after link");
    // Even without the link hook, a new scan never inherits the last scan's 401.
    let rt = relinked_runtime("relink-stale-scan");
    rt.begin_scan();
    rt.set_status("Uploading 2 export(s)…");
    assert!(
        !rt.snapshot(&AppConfig::default()).relink,
        "stale flag leaked"
    );
}

#[test]
fn an_expired_hold_starts_a_scan_on_the_next_tick() {
    assert!(tick_scans(true, false, 1), "the hold just ran out");
    assert!(!tick_scans(false, false, 1), "nothing to do, or still held");
    assert!(tick_scans(false, true, 1), "a file settled");
    assert!(tick_scans(false, false, 300), "periodic rescan");
}

#[test]
fn an_expired_hold_is_taken_exactly_once() {
    let now = Instant::now();
    let later = now + Duration::from_secs(1);
    let mut until = Some(now);
    assert!(take_expired(&mut until, later), "a past hold is taken");
    assert_eq!(until, None, "and cleared");
    assert!(!take_expired(&mut until, later), "only once");
    let live = now + Duration::from_secs(47);
    let mut until = Some(live);
    assert!(!take_expired(&mut until, now), "a live hold is not taken");
    assert_eq!(until, Some(live), "and is kept");

    let rt = Runtime::new(super::temp_dir("take-hold").join("sent.json"));
    assert!(!rt.take_expired_hold(), "no hold, nothing to take");
    rt.hold_for(Duration::ZERO);
    assert_eq!(rt.held_secs(), None, "an expired hold shows no countdown");
    assert!(
        rt.take_expired_hold(),
        "a hold already in the past is taken"
    );
    assert!(!rt.take_expired_hold(), "only once");
}

#[test]
fn a_hold_that_expires_during_a_scan_still_starts_the_next_scan() {
    let rt = Runtime::new(super::temp_dir("hold-mid-scan").join("sent.json"));
    assert!(!tick_scans(rt.take_expired_hold(), false, 1), "idle tick");
    // The scan hits a 429 whose Retry-After runs out before the scan
    // returns, so no tick ever sees the hold live.
    rt.hold_for(Duration::ZERO);
    assert_eq!(rt.held_secs(), None, "no tick samples it as held");
    assert!(
        tick_scans(rt.take_expired_hold(), false, 2),
        "the latched expiry rescans on the next tick"
    );
    assert!(!tick_scans(rt.take_expired_hold(), false, 3), "only once");
}

#[test]
fn the_hold_countdown_is_derived_at_render_time() {
    let now = std::time::Instant::now();
    assert_eq!(secs_left(None, now), None);
    assert_eq!(
        secs_left(Some(now + Duration::from_secs(47)), now),
        Some(47)
    );
    let later = now + Duration::from_secs(20);
    assert_eq!(
        secs_left(Some(now + Duration::from_secs(47)), later),
        Some(27)
    );
    assert_eq!(secs_left(Some(now), later), None, "an expired hold is gone");

    let rt = Runtime::new(super::temp_dir("live-hold").join("sent.json"));
    let cfg = AppConfig::default();
    rt.hold_for(Duration::from_secs(50));
    let device = rt.snapshot(&cfg).device.unwrap_or_default();
    assert!(
        device.starts_with("Rate limited — retrying in"),
        "a live hold shows its countdown: {device}"
    );
    rt.hold_for(Duration::ZERO);
    assert_eq!(
        rt.snapshot(&cfg).device,
        None,
        "no stale countdown once it passes"
    );
}

#[test]
fn baked_text_never_carries_a_countdown() {
    assert_eq!(
        device_note(false, true, Ok(true)).as_deref(),
        Some(RETRYING)
    );
    assert!(device_note(true, true, Ok(true)).is_some_and(|d| d.starts_with("Paused")));
    assert_eq!(device_note(false, false, Ok(true)), None);
    let fx = effect(&UploadOutcome::RateLimited {
        wait: Duration::from_secs(47),
    });
    assert_eq!(fx.hold, Some(Duration::from_secs(47)));
    assert_eq!(shown_text(&fx), RETRYING);
    assert_eq!(
        stopped_line(2, 5, &fx),
        format!("Uploaded 2 of 5 export(s); {RETRYING}"),
        "the batch line"
    );
    let rt = Runtime::new(super::temp_dir("baked-result").join("sent.json"));
    rt.finish(&pending("A", "2026-10-01T00:00:00Z", "a1"), &fx);
    let shown: Vec<String> = rt
        .snapshot(&AppConfig::default())
        .results
        .into_iter()
        .map(|r| r.result)
        .collect();
    assert_eq!(shown, vec![RETRYING.to_owned()], "the per-export result");
    let fx = effect(&UploadOutcome::Deferred {
        reason: "offline".into(),
    });
    assert_eq!(
        shown_text(&fx),
        "Will retry: offline",
        "no hold keeps its text"
    );
}
