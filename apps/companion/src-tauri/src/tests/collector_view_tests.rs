//! ROK-2061 lane 3: per-game rows, device state, folder override and the
//! first-upload preview hold.

use crate::collector::{Blob, PendingUpload};
use crate::collector_view::{apply_root, device_state, rows};
use crate::config::AppConfig;
use crate::manifest::{Platform, BG3_SAVE, WOW_LEDGERLINK};
use crate::preview::{base64, hold};
use crate::runtime::{effect, Runtime};
use crate::upload::UploadOutcome;
use serde_json::json;
use std::collections::BTreeMap;

fn env(k: &str) -> Option<String> {
    (k == "HOME").then(|| "/home/ada".to_owned())
}

fn pending(name: &str, saved_at: &str, sha: &str) -> PendingUpload {
    PendingUpload {
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
    assert_eq!(ids, [WOW_LEDGERLINK.id, BG3_SAVE.id]);
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
    assert!(apply_root(&mut cfg, BG3_SAVE.id, "/", &env)
        .unwrap_err()
        .contains("whole drive"));
    apply_root(&mut cfg, BG3_SAVE.id, " /games/bg3/Story ", &env).expect("absolute is accepted");
    assert_eq!(
        cfg.roots_override(BG3_SAVE.id),
        Some(vec!["/games/bg3/Story".to_owned()])
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
