//! ROK-2061 lane 2: the BG3 save collector, through the registry.

use super::temp_dir;
use crate::bg3_campaigns::{persist_first_sight, CampaignStore};
use crate::bg3_collector::{build, newest_first, Bg3SaveCollector};
use crate::collector::{Collector, Scan, SentState};
use crate::config::AppConfig;
use crate::manifest::{Platform, BG3_SAVE};
use crate::registry::{CollectorRegistry, BUILTINS};
use crate::scope::WatchScope;
use crate::upload::Uploader;
use rl_collectors::sha256_hex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SAVE_INFO: &str = include_str!(
    "../../../../../contracts/bg3-save/v1/fixtures/saveinfo/act2-party-4p-summon.saveinfo.json"
);
const GOLDEN: &str =
    include_str!("../../../../../contracts/bg3-save/v1/fixtures/act2-party-4p-summon.json");
const WEBP: &[u8] =
    include_bytes!("../../../../../contracts/bg3-save/v1/fixtures/placeholder-thumbnail.webp");
const FOLDER: &str = "QuickSave_1-1234567890";
/// The anonymised real-shape `meta.lsf` (ROK-2070 L2): a FixedString GameID
/// replaced by [`SYNTHETIC_GAME_ID`], every other string zeroed or filler.
const META_LSF: &[u8] = include_bytes!(
    "../../../../../crates/rl-collectors/tests/fixtures/meta/real-shape-string.meta.lsf"
);
const SYNTHETIC_GAME_ID: &str = "00000000-2070-4000-8000-000000000001";

/// A synthetic LSPK v18 package of stored entries (the layout `read_lspk`
/// parses: header, data, LZ4 file table last). Never a real user save.
fn lspk(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = vec![0u8; 40];
    let mut table = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u64;
        out.extend_from_slice(data);
        let mut n = [0u8; 256];
        n[..name.len()].copy_from_slice(name.as_bytes());
        table.extend_from_slice(&n);
        table.extend_from_slice(&(offset as u32).to_le_bytes());
        table.extend_from_slice(&((offset >> 32) as u16).to_le_bytes());
        table.extend_from_slice(&[0, 0]);
        table.extend_from_slice(&(data.len() as u32).to_le_bytes());
        table.extend_from_slice(&(data.len() as u32).to_le_bytes());
    }
    let list_offset = out.len() as u64;
    let compressed = lz4_flex::block::compress(&table);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    out[0..4].copy_from_slice(b"LSPK");
    out[4..8].copy_from_slice(&18u32.to_le_bytes());
    out[8..16].copy_from_slice(&list_offset.to_le_bytes());
    out[16..20].copy_from_slice(&(8 + compressed.len() as u32).to_le_bytes());
    out[38..40].copy_from_slice(&1u16.to_le_bytes());
    out
}

fn save_bytes() -> Vec<u8> {
    lspk(&[
        ("SaveInfo.json", SAVE_INFO.as_bytes()),
        ("QuickSave_1.WebP", WEBP),
    ])
}

/// [`save_bytes`] plus the anonymised `meta.lsf`, as a real save carries it.
fn save_bytes_with_meta(meta: &[u8]) -> Vec<u8> {
    lspk(&[
        ("SaveInfo.json", SAVE_INFO.as_bytes()),
        ("QuickSave_1.WebP", WEBP),
        ("meta.lsf", meta),
    ])
}

/// `<story>/<FOLDER>/QuickSave_1.lsv` plus a file the manifest ignores.
fn install(story: &Path, bytes: &[u8]) -> PathBuf {
    let dir = story.join(FOLDER);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("QuickSave_1.WebP"), WEBP).expect("webp");
    let file = dir.join("QuickSave_1.lsv");
    std::fs::write(&file, bytes).expect("lsv");
    file
}

fn registry(story: &Path, on: bool) -> CollectorRegistry {
    registry_ticked(story, on, &[])
}

/// [`registry`] with the picker's ticks, keyed by save-folder name.
fn registry_ticked(story: &Path, on: bool, ticks: &[(&str, bool)]) -> CollectorRegistry {
    let mut cfg = AppConfig::default();
    for (folder, t) in ticks {
        let key = rl_collectors::campaign_key_from_save_folder(folder);
        cfg.bg3_campaigns.insert(key, *t);
    }
    cfg.collector_roots.insert(
        BG3_SAVE.id.into(),
        vec![story.to_string_lossy().into_owned()],
    );
    if on {
        cfg.set_collector_enabled(BG3_SAVE.id, true);
    }
    CollectorRegistry::build(BUILTINS, &cfg, Some(Platform::Windows), &|_| None).expect("builtins")
}

#[test]
fn a_save_normalises_to_the_golden_payload_without_the_raw_folder_name() {
    let (body, blobs, label) = build(&save_bytes(), FOLDER, None).expect("parses");
    let mut golden: Value = serde_json::from_str(GOLDEN).expect("golden");
    let payload = &body["payload"];
    // Collector-supplied facts differ from the fixture's synthetic ones.
    golden["payload"]["savedAt"] = payload["savedAt"].clone();
    golden["payload"]["campaignKey"] = Value::from(sha256_hex(FOLDER.as_bytes()));
    assert_eq!(payload, &golden["payload"], "payload matches the contract");
    assert_eq!(body["contract"], "bg3-save");
    assert_eq!(body["contractVersion"], 1);
    assert_eq!(label, "QuickSave_1");
    assert_eq!(blobs.len(), 1, "the thumbnail goes up as a blob");
    assert_eq!(blobs[0].sha256, sha256_hex(WEBP));
    assert_eq!(payload["thumbnail"]["sha256"], sha256_hex(WEBP));
    assert!(
        !body.to_string().contains(FOLDER),
        "the raw save-folder name never leaves the machine"
    );
}

#[test]
fn a_save_without_a_readable_game_id_keys_on_the_save_folder() {
    // The placeholder save has no meta.lsf; a broken one must not fail either.
    for bytes in [save_bytes(), save_bytes_with_meta(b"LSOF not really")] {
        let (body, _, _) = build(&bytes, FOLDER, None).expect("an upload never fails on meta.lsf");
        let payload = &body["payload"];
        assert_eq!(payload["campaignKeySource"], "save-folder");
        assert_eq!(payload["campaignKey"], sha256_hex(FOLDER.as_bytes()));
    }
}

#[test]
fn a_save_with_a_game_id_keys_on_its_hash() {
    let bytes = save_bytes_with_meta(META_LSF);
    let (body, _, label) = build(&bytes, FOLDER, None).expect("parses");
    let payload = &body["payload"];
    assert_eq!(
        (&payload["campaignKeySource"], &payload["campaignKey"]),
        (
            &Value::from("game-id"),
            &Value::from(sha256_hex(SYNTHETIC_GAME_ID.as_bytes()))
        ),
        "the key is sha256(GameID) with source game-id, not the folder hash"
    );
    assert_eq!(label, "QuickSave_1");
    let text = body.to_string();
    assert!(
        !text.contains(SYNTHETIC_GAME_ID) && !text.contains(FOLDER),
        "neither the raw GameID nor the folder name leaves the machine"
    );
    let mut golden: Value = serde_json::from_str(GOLDEN).expect("golden");
    golden["payload"]["savedAt"] = payload["savedAt"].clone();
    golden["payload"]["campaignKey"] = payload["campaignKey"].clone();
    golden["payload"]["campaignKeySource"] = Value::from("game-id");
    assert_eq!(payload, &golden["payload"], "otherwise the same payload");
}

#[test]
fn bg3_is_off_by_default_and_dedupes_by_sha256_once_on() {
    let story = temp_dir("bg3-story");
    let file = install(&story, &save_bytes());
    let off = registry(&story, false);
    let entry = off.get(BG3_SAVE.id).expect("bg3 registered");
    assert!(!entry.enabled, "opt-in per game");
    assert!(entry
        .scan("https://raid.example.net", &SentState::default())
        .is_none());

    let on = registry(&story, true);
    assert_eq!(on.watch_roots(), vec![story.clone()]);
    assert_eq!(on.wanting(&file).count(), 1, "an .lsv change settles");
    assert_eq!(on.wanting(&file.with_extension("WebP")).count(), 0);
    let bg3 = on.get(BG3_SAVE.id).expect("bg3");
    let origin = "https://raid.example.net";
    let scan = bg3.scan(origin, &SentState::default()).expect("enabled");
    assert_eq!((scan.files, scan.pending.len()), (1, 1), "{:?}", scan.notes);
    let p = &scan.pending[0];
    assert_eq!(p.sha256, sha256_hex(&save_bytes()));
    let mut sent = SentState::default();
    sent.record(&p.scope, &p.item, &p.sha256);
    let again = bg3.scan(origin, &sent).expect("enabled");
    assert_eq!(
        (again.files, again.pending.len()),
        (1, 0),
        "unchanged save not resent"
    );

    let mut changed = save_bytes();
    changed.push(0);
    std::fs::write(&file, &changed).expect("rewrite");
    let third = bg3.scan(origin, &sent).expect("enabled");
    assert_eq!(third.pending.len(), 1, "a changed save is sent again");
    let _ = std::fs::remove_dir_all(story);
}

#[test]
fn a_broken_save_is_noted_not_uploaded() {
    let story = temp_dir("bg3-broken");
    install(&story, b"not an lspk");
    let reg = registry(&story, true);
    let scan = reg
        .get(BG3_SAVE.id)
        .and_then(|e| e.scan("https://raid.example.net", &SentState::default()))
        .expect("enabled");
    assert_eq!(scan.pending.len(), 0);
    assert!(
        scan.notes.iter().any(|n| n.contains("not readable yet")),
        "{:?}",
        scan.notes
    );
    let _ = std::fs::remove_dir_all(story);
}

#[test]
fn default_save_folders_resolve_per_os() {
    let env = |k: &str| match k {
        "LOCALAPPDATA" => Some(r"C:\Users\p\AppData\Local".to_owned()),
        "HOME" => Some("/Users/p".to_owned()),
        _ => None,
    };
    let win = BG3_SAVE.default_roots_for(Some(Platform::Windows), &env);
    assert_eq!(
        win,
        vec![PathBuf::from(
            r"C:\Users\p\AppData\Local\Larian Studios\Baldur's Gate 3\PlayerProfiles\Public\Savegames\Story"
        )]
    );
    let mac = BG3_SAVE.default_roots_for(Some(Platform::Macos), &env);
    assert_eq!(
        mac,
        vec![PathBuf::from(
            "/Users/p/Documents/Larian Studios/Baldur's Gate 3/PlayerProfiles/Public/Savegames/Story"
        )]
    );
    assert!(BG3_SAVE
        .default_roots_for(Some(Platform::Windows), &|_| None)
        .is_empty());
}

#[test]
fn blob_urls_take_only_a_sha256() {
    let up = Uploader::new("https://raid.example.net").expect("url");
    let sha = sha256_hex(WEBP);
    assert_eq!(
        up.blob_url(&sha).expect("hex").as_str(),
        format!("https://raid.example.net/api/companion/v1/blobs/{sha}")
    );
    assert!(up.blob_url("../messages/x").is_err());
    assert!(up.blob_url(&sha.to_uppercase()).is_err());
}

#[test]
fn newest_first_orders_by_mtime_with_unknown_last() {
    let at = |s: u64| Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(s));
    let order = newest_first(vec![
        (PathBuf::from("old"), at(100)),
        (PathBuf::from("none"), None),
        (PathBuf::from("new"), at(300)),
        (PathBuf::from("mid"), at(200)),
    ]);
    let names: Vec<String> = order
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["new", "mid", "old", "none"]);
}

fn stamp(story: &Path, folder: &str, secs: u64) {
    let dir = story.join(folder);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let file = dir.join("QuickSave_1.lsv");
    std::fs::write(&file, save_bytes()).expect("lsv");
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let f = std::fs::File::options()
        .write(true)
        .open(&file)
        .expect("open");
    f.set_modified(at).expect("set mtime");
}

fn queued(reg: &CollectorRegistry, folders: [&'static str; 3]) -> Vec<&'static str> {
    let scan = reg
        .get(BG3_SAVE.id)
        .and_then(|e| e.scan("https://raid.example.net", &SentState::default()))
        .expect("enabled");
    scan.pending
        .iter()
        .filter_map(|p| folders.into_iter().find(|f| p.scope.contains(f)))
        .collect()
}

#[test]
fn an_unticked_campaign_is_not_queued_and_an_old_one_waits_for_a_tick() {
    let story = temp_dir("bg3-picker");
    stamp(&story, "A_old-1", 1_700_000_000);
    stamp(&story, "B_new-2", 4_000_000_000);
    stamp(&story, "C_mid-3", 1_750_000_000);
    let folders = ["A_old", "B_new", "C_mid"];
    // Defaults: B (saved now) on; A and C (over 30 days old) off.
    assert_eq!(queued(&registry(&story, true), folders), ["B_new"]);
    let picked = registry_ticked(&story, true, &[("A_old-1", true), ("B_new-2", false)]);
    assert_eq!(
        queued(&picked, folders),
        ["A_old"],
        "ticks override the default"
    );
    let _ = std::fs::remove_dir_all(story);
}

#[test]
fn a_scan_queues_the_newest_save_first() {
    let story = temp_dir("bg3-newest");
    stamp(&story, "A_old-1", 1_700_000_000);
    stamp(&story, "B_new-2", 1_800_000_000);
    stamp(&story, "C_mid-3", 1_750_000_000);
    // All three ticked: these mtimes are over 30 days old (ROK-2068 picker).
    let ticks = [("A_old-1", true), ("B_new-2", true), ("C_mid-3", true)];
    let on = registry_ticked(&story, true, &ticks);
    let bg3 = on.get(BG3_SAVE.id).expect("bg3");
    let scan = bg3
        .scan("https://raid.example.net", &SentState::default())
        .expect("enabled");
    let folders = ["A_old", "B_new", "C_mid"];
    let order: Vec<&str> = scan
        .pending
        .iter()
        .filter_map(|p| folders.into_iter().find(|f| p.scope.contains(f)))
        .collect();
    assert_eq!(order, ["B_new", "C_mid", "A_old"], "{:?}", scan.notes);
    let _ = std::fs::remove_dir_all(story);
}

/// A bare collector over `story` with `ticks` (by save folder) and its own store.
fn picked(story: &Path, ticks: &[(&str, bool)]) -> (Bg3SaveCollector, &'static CampaignStore) {
    let ticks: BTreeMap<String, bool> = ticks
        .iter()
        .map(|(f, t)| (rl_collectors::campaign_key_from_save_folder(f), *t))
        .collect();
    let store: &'static CampaignStore = Box::leak(Box::new(CampaignStore::new()));
    let bg3 = Bg3SaveCollector::scoped(&[story.to_path_buf()], WatchScope::new(&[story]))
        .with_ticks(ticks)
        .remembering(store);
    (bg3, store)
}

#[test]
fn an_unticked_campaigns_saves_are_listed_but_never_built() {
    let story = temp_dir("bg3-off-unbuilt");
    stamp(&story, "A_off-1", 1_750_000_000);
    stamp(&story, "B_on-2", 1_760_000_000);
    let (bg3, store) = picked(&story, &[("A_off-1", false), ("B_on-2", true)]);
    let off = rl_collectors::campaign_key_from_save_folder("A_off-1");
    let found = bg3.found(
        "https://raid.example.net",
        &SentState::default(),
        &mut Scan::default(),
    );
    let built: Vec<bool> = found
        .iter()
        .filter(|(c, _)| c.key == off)
        .map(|(_, p)| p.is_some())
        .collect();
    assert_eq!(
        built,
        [false],
        "the unticked campaign's save must be found but carry no upload"
    );
    assert_eq!(
        bg3.builds(),
        1,
        "only the ticked campaign's save may reach the builder"
    );
    let scan = bg3.scan("https://raid.example.net", &SentState::default());
    assert_eq!(
        (bg3.builds(), scan.pending.len()),
        (2, 1),
        "{:?}",
        scan.notes
    );
    let row = store.groups().into_iter().find(|g| g.key == off);
    let row = row.expect("the unticked campaign still has a picker row");
    assert_eq!((row.count, row.newest_unix), (1, 1_750_000_000));
    assert!(!row.name.is_empty(), "the row keeps its name");
    let _ = std::fs::remove_dir_all(story);
}

#[test]
fn a_first_scan_saves_new_campaigns_defaults_and_a_second_scan_writes_nothing() {
    let story = temp_dir("bg3-first-sight");
    stamp(&story, "A_old-1", 1_700_000_000);
    stamp(&story, "B_new-2", 4_000_000_000);
    let path = temp_dir("bg3-first-sight-cfg").join("config.json");
    let shared =
        crate::state::Shared::new(path.clone(), Box::new(crate::token::MemoryStore::default()));
    let (bg3, store) = picked(&story, &[]);
    bg3.scan("https://raid.example.net", &SentState::default());
    persist_first_sight(store, &shared);
    let key = rl_collectors::campaign_key_from_save_folder;
    let want = BTreeMap::from([(key("A_old-1"), false), (key("B_new-2"), true)]);
    assert_eq!(
        shared.config().bg3_campaigns,
        want,
        "the 30-day defaults are adopted"
    );
    assert_eq!(
        AppConfig::load(&path).bg3_campaigns,
        want,
        "and saved to disk"
    );
    std::fs::remove_file(&path).expect("drop the saved config");
    let again = Bg3SaveCollector::scoped(std::slice::from_ref(&story), WatchScope::new(&[&story]))
        .with_ticks(shared.config().bg3_campaigns)
        .remembering(store);
    again.scan("https://raid.example.net", &SentState::default());
    persist_first_sight(store, &shared);
    assert!(
        !path.exists(),
        "a scan with no new campaign must not write the config"
    );
    let _ = std::fs::remove_dir_all(story);
}
