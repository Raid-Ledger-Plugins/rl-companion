//! ROK-1966: the BG3 screenshot collector. Every image is generated here —
//! never a real screenshot.

use super::temp_dir;
use crate::bg3_screenshot::{build, Bg3ScreenshotCollector, STEAM_SUBPATH};
use crate::collector::{scope_key, Collector, SentState};
use crate::config::AppConfig;
use crate::manifest::{Platform, BG3_SCREENSHOT};
use crate::registry::{CollectorRegistry, BUILTINS};
use crate::scope::WatchScope;
use crate::screenshot_commands::{hubs_view, valid_hub};
use crate::screenshot_encode::{encode_screenshot, fit_under, WEBP_MAX_BYTES};
use crate::server_config::ServerConfig;
use image::{ImageFormat, RgbImage};
use rl_collectors::{sha256_hex, webp_dimensions};
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ORIGIN: &str = "https://rl.test";
const HUB: i64 = 42;
const OPT_IN: i64 = 1_800_000_000;
const SCHEMA: &str = include_str!("../../../../../contracts/bg3-screenshot/v1/schema.json");
const FIX_1080P: &str =
    include_str!("../../../../../contracts/bg3-screenshot/v1/fixtures/steam-1080p.json");
const FIX_WIDE: &str =
    include_str!("../../../../../contracts/bg3-screenshot/v1/fixtures/ultrawide-downscaled.json");
const INVALID_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../contracts/bg3-screenshot/v1/fixtures/invalid"
);

/// A noisy RGB image (worst case for the encoder), deterministic per `seed`.
fn noise(w: u32, h: u32, seed: u32) -> RgbImage {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    RgbImage::from_fn(w, h, |_, _| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let [r, g, b, _] = x.to_le_bytes();
        image::Rgb([r, g, b])
    })
}

fn png(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    noise(w, h, seed)
        .write_to(&mut out, ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

fn at(unix: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(u64::try_from(unix).expect("positive"))
}

/// Write a screenshot with mtime `unix`.
fn shot(dir: &Path, name: &str, bytes: &[u8], unix: i64) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("write shot");
    let f = std::fs::File::options().write(true).open(&p).expect("open");
    f.set_modified(at(unix)).expect("set mtime");
    p
}

/// `<base>/userdata/123/760/remote/1086940/screenshots`, created.
fn steam(base: &Path) -> (PathBuf, PathBuf) {
    let userdata = base.join("userdata");
    let folder = STEAM_SUBPATH
        .iter()
        .fold(userdata.join("123"), |p, s| p.join(s));
    std::fs::create_dir_all(folder.join("thumbnails")).expect("mkdir");
    (userdata, folder)
}

fn collector(root: &Path, hub: Option<i64>) -> Bg3ScreenshotCollector {
    let roots = vec![root.to_path_buf()];
    Bg3ScreenshotCollector::scoped(&roots, WatchScope::new(&roots))
        .with_opt_in(Some(OPT_IN))
        .with_hub(hub)
}

fn errors(instance: &Value) -> Vec<String> {
    let schema: Value = serde_json::from_str(SCHEMA).expect("schema json");
    let v = jsonschema::validator_for(&schema).expect("schema compiles");
    v.iter_errors(instance).map(|e| e.to_string()).collect()
}

#[test]
fn finds_steam_folder_and_skips_thumbnails_and_old_files() {
    let base = temp_dir("shot-cutoff");
    let (userdata, folder) = steam(&base);
    shot(&folder, "old.png", &png(32, 24, 1), OPT_IN - 10);
    shot(&folder, "new.png", &png(32, 24, 2), OPT_IN + 10);
    shot(&folder, "edge.jpg.txt", b"x", OPT_IN + 10);
    shot(
        &folder.join("thumbnails"),
        "thumb.png",
        &png(8, 8, 3),
        OPT_IN + 10,
    );
    let c = collector(&userdata, Some(HUB));
    assert_eq!(c.folders(), vec![folder.clone()]);
    let scan = c.scan(ORIGIN, &SentState::default());
    let labels: Vec<&str> = scan.pending.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(
        labels,
        vec!["Screenshot new.png"],
        "only the post-opt-in file"
    );
    assert_eq!(scan.files, 1, "the old file is never read");
}

#[test]
fn a_manual_override_names_the_screenshots_folder_itself() {
    let dir = temp_dir("shot-override");
    shot(&dir, "a.png", &png(16, 16, 4), OPT_IN + 1);
    let scan = collector(&dir, Some(HUB)).scan(ORIGIN, &SentState::default());
    assert_eq!(scan.pending.len(), 1);
}

#[test]
fn no_opt_in_time_sends_nothing() {
    let dir = temp_dir("shot-no-optin");
    shot(&dir, "a.png", &png(16, 16, 5), OPT_IN + 1);
    let roots = vec![dir.clone()];
    let c = Bg3ScreenshotCollector::scoped(&roots, WatchScope::new(&roots)).with_hub(Some(HUB));
    let scan = c.scan(ORIGIN, &SentState::default());
    assert!(scan.pending.is_empty());
    assert_eq!(scan.files, 0);
    assert!(
        scan.notes.iter().any(|n| n.contains("no opt-in time")),
        "{:?}",
        scan.notes
    );
}

#[test]
fn dedupes_by_the_original_bytes_sha256() {
    let dir = temp_dir("shot-dedupe");
    let bytes = png(40, 30, 6);
    shot(&dir, "a.png", &bytes, OPT_IN + 1);
    let c = collector(&dir, Some(HUB));
    let first = c.scan(ORIGIN, &SentState::default());
    let p = first.pending.first().expect("one pending");
    let original = sha256_hex(&bytes);
    assert_eq!(p.item, original);
    assert_eq!(p.sha256, original);
    assert_eq!(p.scope, scope_key(ORIGIN, BG3_SCREENSHOT.id));
    assert_ne!(
        p.body["payload"]["image"]["sha256"],
        json!(original),
        "WebP hash differs"
    );
    let mut sent = SentState::default();
    sent.record(&p.scope, &p.item, &p.sha256);
    let again = c.scan(ORIGIN, &sent);
    assert!(
        again.pending.is_empty(),
        "a settled original is never re-sent"
    );
    assert_eq!(c.encodes(), 1, "the settled original is not re-encoded");
}

#[test]
fn no_hub_picked_holds_screenshots_back() {
    let dir = temp_dir("shot-no-hub");
    shot(&dir, "a.png", &png(16, 16, 7), OPT_IN + 1);
    let c = collector(&dir, None);
    let scan = c.scan(ORIGIN, &SentState::default());
    assert!(scan.pending.is_empty(), "nothing uploads without a hub");
    assert_eq!(c.waiting(), 1);
    assert_eq!(c.encodes(), 0, "nothing is encoded either");
    assert!(
        scan.notes.iter().any(|n| n.contains("pick a hub")),
        "{:?}",
        scan.notes
    );
}

#[test]
fn a_built_message_validates_against_the_pinned_schema() {
    let bytes = png(320, 180, 8);
    let (body, blob) = build(&bytes, Some(at(OPT_IN)), HUB).expect("builds");
    assert_eq!(errors(&body), Vec::<String>::new());
    let p = &body["payload"];
    assert_eq!(p["target"], json!({ "hubGuildId": HUB }));
    assert_eq!(
        (p["width"].clone(), p["height"].clone()),
        (json!(320), json!(180))
    );
    assert_eq!(p["image"]["sha256"], json!(blob.sha256));
    assert_eq!(p["image"]["byteLength"], json!(blob.bytes.len()));
    assert_eq!(sha256_hex(&blob.bytes), blob.sha256);
    assert_eq!(blob.content_type, "image/webp");
    let taken = p["takenAt"].as_str().expect("takenAt");
    let captured = body["capturedAt"].as_str().expect("capturedAt");
    assert!(
        taken <= captured,
        "takenAt {taken} after capturedAt {captured}"
    );
}

#[test]
fn pinned_fixtures_agree_with_the_schema() {
    for fixture in [FIX_1080P, FIX_WIDE] {
        let v: Value = serde_json::from_str(fixture).expect("fixture json");
        assert_eq!(errors(&v), Vec::<String>::new());
    }
    // Zod refinements, not expressible in JSON Schema (CONTRACT.md §4).
    let refinement_only = ["image-wider-than-source.json", "taken-after-captured.json"];
    let mut checked = 0;
    for entry in std::fs::read_dir(INVALID_DIR).expect("invalid dir") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        if name == "EXPECTED.json" || refinement_only.contains(&name.as_str()) {
            continue;
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read"))
            .expect("fixture json");
        assert!(!errors(&v).is_empty(), "{name} should fail the schema");
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} invalid fixtures checked");
}

#[test]
fn a_large_noisy_capture_fits_one_mib_and_4096_px() {
    let bytes = png(4800, 1200, 9);
    let enc = encode_screenshot(&bytes).expect("encodes");
    assert!(enc.webp.len() <= WEBP_MAX_BYTES, "{} bytes", enc.webp.len());
    assert!(
        enc.width <= 4096 && enc.height <= 4096,
        "{}x{}",
        enc.width,
        enc.height
    );
    assert!(enc.width <= enc.source_width && enc.height <= enc.source_height);
    assert_eq!((enc.source_width, enc.source_height), (4800, 1200));
    assert_eq!(
        webp_dimensions(&enc.webp).expect("webp header"),
        (enc.width, enc.height)
    );
}

#[test]
fn an_impossible_budget_fails_with_a_clear_error() {
    let err = fit_under(noise(256, 256, 10), 10).expect_err("cannot fit 10 bytes");
    assert!(err.contains("could not fit a WebP under 10 bytes"), "{err}");
    let not_image = encode_screenshot(b"not an image").expect_err("garbage");
    assert!(not_image.contains("image"), "{not_image}");
}

#[test]
fn off_by_default_lists_nothing_and_on_scans_through_the_registry() {
    let home = temp_dir("shot-registry");
    let (_, folder) = steam(&home.join(".local/share/Steam"));
    shot(&folder, "a.png", &png(16, 16, 11), OPT_IN + 1);
    let env = |k: &str| (k == "HOME").then(|| home.to_string_lossy().into_owned());
    let mut cfg = AppConfig::default();
    let off = CollectorRegistry::build(BUILTINS, &cfg, Some(Platform::Linux), &env).expect("ok");
    let entry = off.get(BG3_SCREENSHOT.id).expect("registered");
    assert!(!entry.enabled, "off by default");
    assert!(entry.watch_roots().is_empty(), "no listing before opt-in");
    assert!(
        entry.scan(ORIGIN, &SentState::default()).is_none(),
        "no reads"
    );
    cfg.set_collector_enabled_at(BG3_SCREENSHOT.id, true, OPT_IN);
    cfg.bg3_screenshot_hub = Some(HUB);
    let on = CollectorRegistry::build(BUILTINS, &cfg, Some(Platform::Linux), &env).expect("ok");
    let entry = on.get(BG3_SCREENSHOT.id).expect("registered");
    assert_eq!(entry.watch_roots(), vec![folder]);
    let scan = entry.scan(ORIGIN, &SentState::default()).expect("enabled");
    assert_eq!(scan.pending.len(), 1);
}

#[test]
fn switching_on_stamps_the_opt_in_time_once_per_off_on() {
    let mut cfg = AppConfig::default();
    cfg.set_collector_enabled_at("bg3-screenshot", true, 100);
    cfg.set_collector_enabled_at("bg3-screenshot", true, 200);
    assert_eq!(
        cfg.opted_in_at("bg3-screenshot"),
        Some(100),
        "staying on keeps it"
    );
    cfg.set_collector_enabled_at("bg3-screenshot", false, 300);
    cfg.set_collector_enabled_at("bg3-screenshot", true, 400);
    assert_eq!(
        cfg.opted_in_at("bg3-screenshot"),
        Some(400),
        "off then on restamps"
    );
}

#[test]
fn hub_picker_reads_hub_targets_and_handles_an_older_server() {
    let mut cfg = AppConfig::default();
    assert!(!hubs_view(&cfg, 0).server_lists_hubs);
    let sc: ServerConfig = serde_json::from_value(json!({ "contracts": [
        { "id": "bg3-screenshot", "enabled": true, "hubTargets": [
            { "hubGuildId": 7, "name": "Party", "isDefault": true } ] },
        { "id": "bg3-save", "enabled": true }
    ]}))
    .expect("config parses");
    cfg.last_server_config = Some(sc);
    cfg.bg3_screenshot_hub = Some(7);
    let view = hubs_view(&cfg, 3);
    assert!(view.server_lists_hubs);
    assert_eq!(view.targets.len(), 1);
    assert!(view
        .targets
        .first()
        .is_some_and(|t| t.is_default && t.hub_guild_id == 7));
    assert_eq!((view.selected, view.waiting), (Some(7), 3));
    assert!(valid_hub(1) && valid_hub(2_147_483_647));
    assert!(!valid_hub(0) && !valid_hub(2_147_483_648));
}
