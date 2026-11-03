//! ROK-2061 review fixes: disabled collectors touch nothing, root overrides
//! must be absolute, and the manifest's `file_patterns` decide relevance.

use super::temp_dir;
use crate::collector::{Collector, Scan, SentState};
use crate::config::AppConfig;
use crate::manifest::{resolve_override, CollectorManifest, Platform, WOW_LEDGERLINK};
use crate::registry::{Builtin, CollectorRegistry, BUILTINS};
use crate::runtime::status_line;
use crate::scope::WatchScope;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Every filesystem-touching call the fake collector receives.
static FS_CALLS: AtomicUsize = AtomicUsize::new(0);

struct Fake;

impl Collector for Fake {
    fn id(&self) -> &'static str {
        "fake-game"
    }
    fn watch_roots(&self) -> Vec<PathBuf> {
        FS_CALLS.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }
    fn scan(&self, _: &str, _: &SentState) -> Scan {
        FS_CALLS.fetch_add(1, Ordering::SeqCst);
        Scan::default()
    }
}

fn make_fake(_: &[PathBuf], _: WatchScope, _: &AppConfig) -> Box<dyn Collector> {
    Box::new(Fake)
}

const FAKE: CollectorManifest = CollectorManifest {
    id: "fake-game",
    display_name: "Fake",
    contract_id: "fake-game",
    default_roots: &[],
    file_patterns: &["*.lsv"],
    ..WOW_LEDGERLINK
};

const FAKE_BUILTINS: &[Builtin] = &[Builtin {
    manifest: &FAKE,
    make: make_fake,
}];

fn home() -> &'static str {
    if cfg!(windows) {
        r"C:\Users\tester"
    } else {
        "/home/tester"
    }
}

fn env(k: &str) -> Option<String> {
    (k == "HOME" || k == "USERPROFILE").then(|| home().to_owned())
}

fn fake_registry(cfg: &AppConfig) -> CollectorRegistry {
    CollectorRegistry::build(FAKE_BUILTINS, cfg, Some(Platform::Linux), &env)
        .expect("fake manifest is valid")
}

#[test]
fn a_disabled_collector_is_never_scanned_listed_or_watched() {
    let root = temp_dir("gating-off");
    std::fs::write(root.join("a.lsv"), b"x").expect("write");
    let mut cfg = AppConfig::default();
    cfg.collector_roots.insert(
        "fake-game".into(),
        vec![root.to_string_lossy().into_owned()],
    );
    let before = FS_CALLS.load(Ordering::SeqCst);
    let reg = fake_registry(&cfg);
    let entry = reg.get("fake-game").expect("fake");
    assert!(!entry.enabled, "opt-in: off by default");
    assert!(entry
        .scan("https://raid.example.net", &SentState::default())
        .is_none());
    assert_eq!(reg.watch_roots(), Vec::<PathBuf>::new());
    assert_eq!(reg.wanting(&root.join("a.lsv")).count(), 0);
    assert_eq!(
        FS_CALLS.load(Ordering::SeqCst) - before,
        0,
        "a disabled collector's scan/watch_roots must not run"
    );
    assert_eq!(
        status_line(&reg, &[]),
        "Uploads are off — turn on Fake uploads"
    );

    cfg.set_collector_enabled("fake-game", true);
    let reg = fake_registry(&cfg);
    let entry = reg.get("fake-game").expect("fake");
    assert!(entry
        .scan("https://raid.example.net", &SentState::default())
        .is_some());
    assert_eq!(reg.wanting(&root.join("a.lsv")).count(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn relevance_comes_from_the_manifest_file_patterns() {
    let root = temp_dir("gating-patterns");
    for name in ["Save.LSV", "notes.txt"] {
        std::fs::write(root.join(name), b"x").expect("write");
    }
    let mut cfg = AppConfig::default();
    cfg.collector_roots.insert(
        "fake-game".into(),
        vec![root.to_string_lossy().into_owned()],
    );
    cfg.set_collector_enabled("fake-game", true);
    let reg = fake_registry(&cfg);
    assert_eq!(reg.wanting(&root.join("Save.LSV")).count(), 1, "any case");
    assert_eq!(reg.wanting(&root.join("notes.txt")).count(), 0);
    // Every shipped manifest names the collector it builds.
    for b in BUILTINS {
        let c = (b.make)(&[], WatchScope::default(), &AppConfig::default());
        assert_eq!(c.id(), b.manifest.id, "manifest id = collector id");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn root_overrides_must_be_absolute_folders() {
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(
        resolve_override("~/Saves", &env),
        Ok(PathBuf::from(format!("{}/Saves", home()))),
        "~ expands"
    );
    let bad = [
        "Saves",
        "~foo/Saves",
        "%UNSET%/x",
        "/",
        "",
        "/tmp/..",
        "~/../..",
        "/a/../b",
    ];
    for raw in bad {
        assert!(
            resolve_override(raw, &env).is_err(),
            "{raw:?} must be refused"
        );
    }
    if cfg!(windows) {
        assert!(resolve_override(r"C:\", &env).is_err(), "whole drive");
    }
    let abs = format!("{}{sep}games", home());
    assert_eq!(resolve_override(&abs, &env), Ok(PathBuf::from(&abs)));

    // Through the registry: a relative override is dropped with a note and
    // the collector gets no roots (no silent fall back).
    let mut cfg = AppConfig::default();
    cfg.collector_roots
        .insert("fake-game".into(), vec!["Saves".into()]);
    let reg = fake_registry(&cfg);
    assert!(reg.get("fake-game").expect("fake").roots.is_empty());
    assert!(
        reg.notes.iter().any(|n| n.contains("must be a full path")),
        "noted: {:?}",
        reg.notes
    );
    let cfg = AppConfig {
        wow_dir: "World of Warcraft".into(),
        ..AppConfig::default()
    };
    let reg =
        CollectorRegistry::build(BUILTINS, &cfg, Some(Platform::Windows), &env).expect("builtins");
    assert!(reg.get(WOW_LEDGERLINK.id).expect("wow").roots.is_empty());
}

#[test]
fn the_window_names_only_what_is_off_when_nothing_is_on() {
    let reg = fake_registry(&AppConfig::default());
    assert_eq!(status_line(&reg, &["Up to date".into()]), "Up to date");
}
