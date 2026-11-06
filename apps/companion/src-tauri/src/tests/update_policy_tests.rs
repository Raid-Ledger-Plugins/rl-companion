//! In-app update rules (update_policy.rs) plus the idle signals the updater
//! reads from the runtime and the dev bridge.

use crate::dev_bridge::server::ServerCtx;
use crate::runtime::Runtime;
use crate::update_policy::*;
use crate::updater::take_marker;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn v(s: &str) -> semver::Version {
    semver::Version::parse(s).expect("semver")
}

#[test]
fn a_test_build_follows_the_test_channel_and_a_plain_build_stable() {
    assert_eq!(Channel::for_version(&v("0.1.0-test.6")), Channel::Test);
    assert_eq!(Channel::for_version(&v("0.2.0-rc.1")), Channel::Test);
    assert_eq!(Channel::for_version(&v("0.1.0")), Channel::Stable);
    assert_eq!(Channel::for_version(&v("1.4.2+build.7")), Channel::Stable);
}

#[test]
fn each_channel_reads_latest_json_from_its_own_rolling_release() {
    assert_eq!(
        Channel::Test.endpoint(),
        "https://github.com/Raid-Ledger-Plugins/rl-companion/releases/download/updates-test/latest.json"
    );
    assert_eq!(
        Channel::Stable.endpoint(),
        "https://github.com/Raid-Ledger-Plugins/rl-companion/releases/download/updates-stable/latest.json"
    );
    assert!(url::Url::parse(&Channel::Test.endpoint()).is_ok());
}

#[test]
fn semver_orders_test_builds_before_the_release_they_lead_to() {
    // The workflow stamps the tag's version into the build; the updater only
    // installs a strictly newer version, so these orderings are what make
    // test.5 -> test.6 -> 0.1.0 -> 0.2.0-test.1 work.
    assert!(v("0.1.0-test.6") > v("0.1.0-test.5"));
    assert!(v("0.1.0-test.10") > v("0.1.0-test.9"));
    assert!(v("0.1.0") > v("0.1.0-test.10"));
    assert!(v("0.2.0-test.1") > v("0.1.0"));
}

fn idle() -> IdleInputs {
    IdleInputs::default()
}

#[test]
fn installs_on_its_own_only_when_nothing_is_in_flight() {
    assert!(is_idle(&idle()), "fresh app, bridge never used");
    assert!(
        !is_idle(&IdleInputs {
            scanning: true,
            ..idle()
        }),
        "upload running"
    );
    assert!(
        !is_idle(&IdleInputs {
            window_visible: true,
            ..idle()
        }),
        "window open"
    );
    let recent = IdleInputs {
        since_bridge_request: Some(BRIDGE_QUIET - Duration::from_secs(1)),
        ..idle()
    };
    assert!(!is_idle(&recent), "dev bridge answered a request just now");
    let quiet = IdleInputs {
        since_bridge_request: Some(BRIDGE_QUIET),
        ..idle()
    };
    assert!(is_idle(&quiet), "dev bridge quiet long enough");
}

#[test]
fn checks_every_six_hours_and_retries_an_hour_after_a_failure() {
    assert_eq!(next_check_in(true), Duration::from_secs(6 * 3600));
    assert_eq!(next_check_in(false), Duration::from_secs(3600));
    assert!(FIRST_CHECK_DELAY <= Duration::from_secs(120));
}

#[test]
fn download_percent_is_clamped_and_absent_without_a_size() {
    assert_eq!(percent(0, Some(200)), Some(0));
    assert_eq!(percent(50, Some(200)), Some(25));
    assert_eq!(percent(250, Some(200)), Some(100));
    assert_eq!(percent(10, None), None);
    assert_eq!(percent(10, Some(0)), None);
}

#[test]
fn the_tray_item_follows_the_phase() {
    let ready = Phase::Ready {
        version: "0.1.0-test.6".into(),
    };
    assert_eq!(tray_label(&Phase::Idle), ("Check for updates".into(), true));
    assert_eq!(
        tray_label(&ready),
        ("Update to 0.1.0-test.6 and restart".into(), true)
    );
    assert!(!tray_label(&Phase::Checking).1);
    assert!(
        !tray_label(&Phase::Installing {
            version: "x".into()
        })
        .1
    );
    let dl = Phase::Downloading {
        version: "x".into(),
        percent: Some(3),
    };
    assert!(!tray_label(&dl).1);
}

#[test]
fn a_click_installs_when_ready_checks_otherwise_and_never_doubles_up() {
    assert_eq!(
        click_action(&Phase::Ready {
            version: "x".into()
        }),
        Action::InstallNow
    );
    assert_eq!(click_action(&Phase::UpToDate), Action::Check);
    assert_eq!(
        click_action(&Phase::Error {
            message: "x".into()
        }),
        Action::Check
    );
    assert_eq!(click_action(&Phase::Checking), Action::Nothing);
    assert_eq!(
        click_action(&Phase::Installing {
            version: "x".into()
        }),
        Action::Nothing
    );
}

#[test]
fn the_status_serialises_flat_for_the_window() {
    let s = UpdateStatus {
        current_version: "0.1.0-test.5".into(),
        channel: Channel::Test,
        phase: Phase::Downloading {
            version: "0.1.0-test.6".into(),
            percent: Some(40),
        },
        last_checked: None,
    };
    assert_eq!(
        serde_json::to_value(&s).expect("json"),
        json!({
            "currentVersion": "0.1.0-test.5",
            "channel": "test",
            "phase": "downloading",
            "version": "0.1.0-test.6",
            "percent": 40,
            "lastChecked": null,
        })
    );
    let up = UpdateStatus {
        phase: Phase::UpToDate,
        ..s
    };
    assert_eq!(
        serde_json::to_value(&up).expect("json")["phase"],
        "upToDate"
    );
}

#[test]
fn the_silent_restart_marker_is_taken_exactly_once() {
    let dir = std::env::temp_dir().join(format!("rl-upd-marker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let m = dir.join(SILENT_RESTART_MARKER);
    assert!(!take_marker(&m), "no marker, no silent restart");
    std::fs::write(&m, b"").expect("write");
    assert!(take_marker(&m));
    assert!(!take_marker(&m), "taken once");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_runtime_reports_a_scan_only_while_its_guard_lives() {
    let dir = std::env::temp_dir().join(format!("rl-upd-rt-{}", std::process::id()));
    let rt = Runtime::new(dir.join("sent.json"));
    assert!(!rt.is_scanning());
    {
        let _g = rt.scan_guard();
        assert!(rt.is_scanning());
    }
    assert!(!rt.is_scanning());
}

#[test]
fn the_dev_bridge_remembers_when_it_last_answered() {
    let ctx = ServerCtx::default();
    let t0 = Instant::now();
    assert_eq!(ctx.since_admitted(t0), None);
    ctx.note_admitted(t0);
    assert_eq!(
        ctx.since_admitted(t0 + Duration::from_secs(7)),
        Some(Duration::from_secs(7))
    );
}

fn conf() -> Value {
    serde_json::from_str(include_str!("../../tauri.conf.json")).expect("tauri.conf.json")
}

#[test]
fn the_updater_installs_passively_and_ordinary_builds_need_no_signing_key() {
    let c = conf();
    let up = &c["plugins"]["updater"];
    assert_eq!(
        up["windows"]["installMode"], "passive",
        "no clicks, no admin prompt"
    );
    // Endpoints are chosen per channel at runtime, never baked in.
    assert!(up.get("endpoints").is_none());
    // CI's plain `tauri build` must not need TAURI_SIGNING_PRIVATE_KEY; only
    // release.yml turns updater artifacts on.
    assert_ne!(c["bundle"]["createUpdaterArtifacts"], json!(true));
}

#[test]
fn the_updater_trusts_a_real_minisign_public_key() {
    // tauri's pubkey is base64 of a minisign public-key file: an
    // "untrusted comment" line, then the key ("RW" + base64 of Ed + key id + 32 bytes).
    use base64::Engine;
    let c = conf();
    let pubkey = c["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("pubkey string");
    assert!(!pubkey.is_empty(), "plugins.updater.pubkey must be set");
    let raw = base64::engine::general_purpose::STANDARD
        .decode(pubkey)
        .expect("pubkey is base64");
    let text = String::from_utf8(raw).expect("utf-8");
    let mut lines = text.lines();
    assert!(lines
        .next()
        .is_some_and(|l| l.starts_with("untrusted comment:")));
    let key = base64::engine::general_purpose::STANDARD
        .decode(lines.next().expect("key line").trim())
        .expect("key is base64");
    assert_eq!(key.len(), 42, "Ed25519 minisign public key");
    assert_eq!(&key[..2], b"Ed");
}
