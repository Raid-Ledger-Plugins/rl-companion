//! ROK-1966 Codex fix lane A: a screenshot's terminal answers (bg3-screenshot/v1
//! §3), live consent during a batch, folder aliases and the capped re-read.

use super::*;
use crate::bg3_screenshot::{
    is_bg3_folder, read_capped, still_opted_in, Bg3ScreenshotCollector, Unready, STEAM_SUBPATH,
};
use crate::blob_send::send_with_blobs;
use crate::collector::{Blob, PendingUpload};
use crate::config::AppConfig;
use crate::manifest::BG3_SCREENSHOT;
use crate::runtime::{effect, GALLERY_FULL_TEXT};
use crate::scope::WatchScope;
use crate::tests::upload_tests::token;
use crate::upload::{classify_message, is_unknown_contract, Attempt, UploadOutcome, Uploader};
use std::path::{Path, PathBuf};

const SHOT: &str = "bg3-screenshot";
const SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MESSAGE: &str = "/api/companion/v1/messages/bg3-screenshot";

fn body(code: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "code": code, "message": "x" })).expect("json")
}

fn outcome(contract: &str, status: u16, code: &str) -> UploadOutcome {
    match classify_message(contract, status, None, &body(code)) {
        Attempt::Done(o) => o,
        Attempt::Retry { reason } => panic!("{status} {code} retried: {reason}"),
    }
}

fn rejected(status: u16, code: &str) -> UploadOutcome {
    UploadOutcome::Rejected {
        status,
        code: code.into(),
        issue: None,
    }
}

#[test]
fn a_full_gallery_settles_the_screenshot_and_tells_the_user() {
    let o = outcome(SHOT, 413, "blob_quota_exceeded");
    assert_eq!(o, rejected(413, "blob_quota_exceeded"));
    let fx = effect(&o);
    assert!(fx.record, "a full Gallery must settle the screenshot");
    assert!(fx.stop, "the rest of the batch waits for a later scan");
    assert_eq!(fx.text, GALLERY_FULL_TEXT);
}

#[test]
fn an_unknown_screenshot_4xx_is_terminal_but_rate_limited_stays_queued() {
    assert_eq!(
        outcome(SHOT, 403, "not_a_hub_member"),
        rejected(403, "not_a_hub_member")
    );
    assert!(effect(&outcome(SHOT, 422, "something_new")).record);
    let limited = outcome(SHOT, 429, "rate_limited");
    assert!(matches!(limited, UploadOutcome::RateLimited { .. }));
    assert!(!effect(&limited).record, "rate_limited stays queued");
}

#[test]
fn a_screenshot_route_not_served_yet_still_defers() {
    assert!(is_unknown_contract(&outcome(SHOT, 404, "unknown_contract")));
    let proxy = classify_message(SHOT, 404, None, b"<html>not found</html>");
    assert!(matches!(
        proxy,
        Attempt::Done(UploadOutcome::Deferred { .. })
    ));
}

#[test]
fn other_contracts_keep_the_shared_rules() {
    for contract in ["bg3-save", "wow-ledgerlink"] {
        let o = outcome(contract, 413, "blob_quota_exceeded");
        assert!(
            matches!(o, UploadOutcome::Deferred { .. }),
            "{contract}: {o:?}"
        );
    }
}

fn shot_pending() -> PendingUpload {
    PendingUpload {
        campaign: None,
        contract: SHOT,
        scope: "scope".into(),
        item: "item".into(),
        sha256: "orig".into(),
        label: "shot.png".into(),
        body: json!({ "contract": SHOT, "payload": { "image": { "sha256": SHA } } }),
        blobs: vec![Blob {
            sha256: SHA.into(),
            content_type: "image/webp".into(),
            bytes: vec![1, 2, 3],
        }],
    }
}

async fn send(server: &MockServer, blob: ResponseTemplate, post: ResponseTemplate) -> UploadOutcome {
    let blob_path = format!("/api/companion/v1/blobs/{SHA}");
    Mock::given(method("PUT"))
        .and(path(blob_path))
        .respond_with(blob)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(MESSAGE))
        .respond_with(post)
        .mount(server)
        .await;
    let up = Uploader::new(&server.uri()).expect("mock URL");
    send_with_blobs(&up, &token(), &shot_pending(), Clock::default().sleeper()).await
}

fn quota() -> ResponseTemplate {
    ResponseTemplate::new(413).set_body_json(json!({ "code": "blob_quota_exceeded", "message": "x" }))
}

#[tokio::test]
async fn a_413_on_the_message_post_settles_the_screenshot() {
    let server = MockServer::start().await;
    let o = send(&server, ResponseTemplate::new(201), quota()).await;
    assert_eq!(o, rejected(413, "blob_quota_exceeded"));
}

#[tokio::test]
async fn a_413_on_the_image_blob_settles_it_without_posting() {
    let server = MockServer::start().await;
    let o = send(&server, quota(), ResponseTemplate::new(202)).await;
    assert_eq!(o, rejected(413, "blob_quota_exceeded"));
    let posts = server.received_requests().await.unwrap_or_default();
    assert!(posts.iter().all(|r| r.url.path() != MESSAGE), "no POST");
}

fn opted_in_at(now: i64) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.set_collector_enabled_at(BG3_SCREENSHOT.id, true, now);
    cfg
}

#[test]
fn consent_is_rechecked_against_the_live_switch_and_opt_in() {
    let scanned = opted_in_at(1_000);
    assert!(still_opted_in(&scanned, &scanned.clone()));
    let mut off = scanned.clone();
    off.set_collector_enabled_at(BG3_SCREENSHOT.id, false, 2_000);
    assert!(!still_opted_in(&scanned, &off), "switched off mid-batch");
    let mut again = off.clone();
    again.set_collector_enabled_at(BG3_SCREENSHOT.id, true, 3_000);
    assert!(!still_opted_in(&scanned, &again), "off then on: new opt-in");
    assert!(!still_opted_in(&AppConfig::default(), &AppConfig::default()));
}

fn steam_dir(userdata: &Path, user: &str, app: &str) -> PathBuf {
    let dir = userdata.join(user).join("760").join("remote").join(app);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[cfg(unix)]
#[test]
fn a_bg3_folder_aliased_to_another_game_is_refused() {
    let base = temp_dir("shot-alias");
    let userdata = base.join("userdata");
    let other = steam_dir(&userdata, "123", "999999").join("screenshots");
    std::fs::create_dir_all(&other).expect("mkdir");
    std::fs::write(other.join("other.png"), b"x").expect("write");
    let bg3 = steam_dir(&userdata, "123", "1086940");
    std::os::unix::fs::symlink(&other, bg3.join("screenshots")).expect("symlink");
    let roots = vec![userdata.clone()];
    let c = Bg3ScreenshotCollector::scoped(&roots, WatchScope::new(&roots));
    assert_eq!(c.folders(), Vec::<PathBuf>::new(), "alias must not be a folder");
    assert!(c.files().is_empty());
    let real = STEAM_SUBPATH.iter().fold(userdata.join("456"), |p, s| p.join(s));
    std::fs::create_dir_all(&real).expect("mkdir");
    assert!(is_bg3_folder(&userdata, &real));
    assert_eq!(c.folders(), vec![real]);
}

#[test]
fn the_send_time_reread_is_capped() {
    let dir = temp_dir("shot-capped");
    let file = dir.join("a.png");
    std::fs::write(&file, [7u8; 11]).expect("write");
    let scope = WatchScope::new(&[dir.clone()]);
    assert!(matches!(
        read_capped(&scope, &file, 10),
        Err(Unready::Retry(e)) if e.contains("over 64 MiB")
    ));
    assert_eq!(read_capped(&scope, &file, 11).expect("fits").len(), 11);
}
