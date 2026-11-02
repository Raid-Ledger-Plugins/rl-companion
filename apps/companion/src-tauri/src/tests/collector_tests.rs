use super::upload_tests::{token, INGEST};
use super::*;
use crate::collector::{Collector, SentState};
use crate::upload::{UploadOutcome, Uploader};
use crate::wow_collector::{captured_at, LedgerLinkCollector, CONTRACT};
use std::path::{Path, PathBuf};
use wiremock::matchers::header;

pub(super) const MULTI: &str =
    include_str!("../../../../../crates/rl-collectors/tests/fixtures/wow_sv/multi-entry.lua");

const PROD: &str = "https://raid.example.net";
const SLOT: &str = "https://slot-1.example.net";

fn outbox_len(text: &str) -> usize {
    rl_collectors::parse_ledgerlink_outbox(text.as_bytes())
        .expect("fixture parses")
        .entries
        .len()
}

/// `<root>/_classic_era_/WTF/Account/<account>/SavedVariables/LedgerLink.lua`.
pub(super) fn install(root: &Path, account: &str, text: &str) -> PathBuf {
    let sv = root
        .join("_classic_era_")
        .join("WTF")
        .join("Account")
        .join(account)
        .join("SavedVariables");
    std::fs::create_dir_all(&sv).expect("mkdir");
    let file = sv.join("LedgerLink.lua");
    std::fs::write(&file, text).expect("write sv");
    // Other addons' files and a backup sit next to it.
    std::fs::write(sv.join("Details.lua"), "Details = {}").expect("write other");
    std::fs::write(sv.join("LedgerLink.lua.bak"), text).expect("write bak");
    file
}

#[test]
fn dedupe_survives_a_restart_and_a_changed_export_is_resent() {
    let root = temp_dir("dedupe");
    let file = install(&root, "12345678#1", MULTI);
    let state_file = root.join("app-data").join("sent-state.json");
    let collector = LedgerLinkCollector::new(&root);
    assert_eq!(
        collector.watch_roots(),
        vec![root.join("_classic_era_").join("WTF")]
    );

    let first = collector.scan(PROD, &SentState::load(&state_file));
    let n = outbox_len(MULTI);
    assert!(n > 0, "fixture has outbox entries");
    assert_eq!(first.files, 1, "the .bak and other addons are ignored");
    assert_eq!(first.pending.len(), n, "first run uploads every entry");

    let mut sent = SentState::default();
    for p in &first.pending {
        sent.record(&p.scope, &p.item, &p.sha256);
    }
    sent.save(&state_file).expect("save state");

    // "Restart": a fresh collector and state read back from disk.
    let reloaded = SentState::load(&state_file);
    assert_eq!(reloaded, sent);
    let again = LedgerLinkCollector::new(&root).scan(PROD, &reloaded);
    assert!(
        again.pending.is_empty(),
        "unchanged exports are not resent: {:?}",
        again.pending.iter().map(|p| &p.item).collect::<Vec<_>>()
    );

    // One export changes (a new logout export for the first character).
    let changed = MULTI.replacen("!RL1!char!eJ", "!RL1!char!eK", 1);
    assert_ne!(changed, MULTI, "fixture edit applied");
    std::fs::write(&file, &changed).expect("rewrite");
    let third = LedgerLinkCollector::new(&root).scan(PROD, &reloaded);
    assert_eq!(third.pending.len(), 1, "only the changed entry is pending");
    let _ = std::fs::remove_dir_all(root);
}

/// Review MAJOR 1: an export a test server settled must still go to prod, and
/// switching back to the test server keeps its history.
#[test]
fn sent_state_is_kept_per_server_origin() {
    let root = temp_dir("origin");
    install(&root, "12345678#1", MULTI);
    let collector = LedgerLinkCollector::new(&root);
    let on_slot = collector.scan(SLOT, &SentState::default());
    assert!(!on_slot.pending.is_empty(), "fixture has outbox entries");
    let mut sent = SentState::default();
    for p in &on_slot.pending {
        assert!(p.scope.starts_with(SLOT), "{}", p.scope);
        sent.record(&p.scope, &p.item, &p.sha256);
    }
    assert!(
        collector.scan(SLOT, &sent).pending.is_empty(),
        "the test server already settled every export"
    );
    assert_eq!(
        collector.scan(PROD, &sent).pending.len(),
        on_slot.pending.len(),
        "prod has settled nothing yet, so every export is sent there"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn missing_wow_folder_scans_to_nothing() {
    let collector = LedgerLinkCollector::new(temp_dir("nowow").join("absent"));
    let scan = collector.scan(PROD, &SentState::default());
    assert_eq!((scan.files, scan.pending.len()), (0, 0));
    assert!(collector.watch_roots().is_empty());
}

#[test]
fn captured_at_is_iso_utc_and_never_in_the_future() {
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    let stamp = captured_at(Some(future));
    assert!(
        stamp.ends_with('Z') && stamp.len() == 20,
        "ISO-8601 UTC seconds: {stamp}"
    );
    assert!(
        stamp <= captured_at(None),
        "a future mtime is clamped to now"
    );
}

/// Linear AC: scan a real folder, upload to a local mock ingest, and check
/// the contract path, the bearer header and the exact envelope shape.
#[tokio::test]
async fn uploads_a_scanned_export_to_the_mock_ingest() {
    let root = temp_dir("ingest");
    install(&root, "ACCOUNT#1", MULTI);
    let collector = LedgerLinkCollector::new(&root);
    let scan = collector.scan(PROD, &SentState::default());
    let pending = scan.pending.first().expect("one pending upload").clone();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(INGEST))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "status": "accepted" })))
        .expect(1)
        .mount(&server)
        .await;
    let clock = Clock::default();
    let outcome = Uploader::new(&server.uri())
        .expect("mock URL")
        .send(&token(), pending.contract, &pending.body, clock.sleeper())
        .await;
    assert_eq!(
        outcome,
        UploadOutcome::Accepted,
        "bearer header + path matched"
    );

    let reqs = server.received_requests().await.unwrap_or_default();
    let req = reqs.first().expect("one request");
    assert_eq!(req.url.path(), "/api/companion/v1/messages/wow-ledgerlink");
    let sent: Value = serde_json::from_slice(&req.body).expect("JSON body");
    let mut keys: Vec<&str> = sent
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "appVersion",
            "capturedAt",
            "collectorVersion",
            "contract",
            "contractVersion",
            "envelopeVersion",
            "payload"
        ]
    );
    assert_eq!(sent["envelopeVersion"], json!(1));
    assert_eq!(sent["contract"], json!(CONTRACT));
    assert_eq!(sent["contractVersion"], json!(1));
    assert_eq!(sent["appVersion"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(
        sent["collectorVersion"],
        json!(rl_collectors::COLLECTOR_VERSION)
    );
    assert!(sent["capturedAt"]
        .as_str()
        .is_some_and(|s| s.ends_with('Z')));
    let payload = sent["payload"].as_object().expect("payload object");
    assert_eq!(payload.len(), 1, "payload is strict: only exportString");
    let text = payload["exportString"].as_str().expect("exportString");
    assert!(
        text.starts_with("!RL1!"),
        "the export text is sent verbatim"
    );
    let original = rl_collectors::parse_ledgerlink_outbox(MULTI.as_bytes()).expect("parse");
    assert!(original
        .entries
        .iter()
        .any(|e| e.text == text && e.guid == pending.item));
    let _ = std::fs::remove_dir_all(root);
}
