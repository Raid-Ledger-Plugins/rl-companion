//! ROK-2061: manifests, the collector registry and the config migration.

use super::collector_tests::{install, MULTI};
use super::temp_dir;
use crate::collector::SentState;
use crate::config::AppConfig;
use crate::manifest::{
    glob_match, resolve_root, validate_all, CollectorManifest, ManifestError, Platform, RootSpec,
    WOW_LEDGERLINK,
};
use crate::registry::{Builtin, CollectorRegistry, BUILTINS};
use std::path::PathBuf;

fn no_env(_: &str) -> Option<String> {
    None
}

fn build(cfg: &AppConfig) -> CollectorRegistry {
    CollectorRegistry::build(BUILTINS, cfg, Some(Platform::Windows), &no_env)
        .expect("builtins are valid")
}

#[test]
fn shipped_manifests_are_valid_and_match_the_contract_ids() {
    let manifests: Vec<_> = BUILTINS.iter().map(|b| b.manifest).collect();
    assert_eq!(validate_all(&manifests), Ok(()));
    assert_eq!(WOW_LEDGERLINK.contract_id, crate::wow_collector::CONTRACT);
    assert_eq!(
        WOW_LEDGERLINK.contract_version,
        crate::wow_collector::CONTRACT_VERSION
    );
    assert_eq!(
        WOW_LEDGERLINK.settle_samples(),
        rl_collectors::DEFAULT_STABLE_SAMPLES,
        "WoW settles exactly as before the registry"
    );
    let here = WOW_LEDGERLINK.default_roots_for(Platform::current(), &no_env);
    let expected = crate::config::default_wow_dir();
    if !expected.is_empty() {
        assert_eq!(
            here,
            vec![PathBuf::from(expected)],
            "same default as the settings"
        );
    }
}

#[test]
fn manifest_validation_rejects_each_bad_field() {
    let ok = WOW_LEDGERLINK;
    let cases: Vec<(CollectorManifest, ManifestError)> = vec![
        (
            CollectorManifest { id: "Wow_LL", ..ok },
            ManifestError::BadId("Wow_LL".into()),
        ),
        (
            CollectorManifest {
                contract_id: "",
                ..ok
            },
            ManifestError::BadContract("wow-ledgerlink", ""),
        ),
        (
            CollectorManifest {
                contract_version: 0,
                ..ok
            },
            ManifestError::BadVersion("wow-ledgerlink"),
        ),
        (
            CollectorManifest {
                display_name: " ",
                ..ok
            },
            ManifestError::NoName("wow-ledgerlink"),
        ),
        (
            CollectorManifest {
                file_patterns: &[],
                ..ok
            },
            ManifestError::NoPatterns("wow-ledgerlink"),
        ),
        (
            CollectorManifest {
                default_roots: &[RootSpec {
                    platform: Platform::Linux,
                    path: "",
                }],
                ..ok
            },
            ManifestError::EmptyRoot("wow-ledgerlink"),
        ),
        (
            CollectorManifest {
                settle_ms: 500,
                ..ok
            },
            ManifestError::BadSettle("wow-ledgerlink", 500),
        ),
    ];
    for (m, want) in cases {
        assert_eq!(m.validate(), Err(want));
    }
    assert_eq!(
        validate_all(&[&ok, &ok]),
        Err(ManifestError::Duplicate("wow-ledgerlink"))
    );
}

#[test]
fn an_invalid_builtin_fails_registry_construction() {
    static BAD: CollectorManifest = CollectorManifest {
        settle_ms: 0,
        ..WOW_LEDGERLINK
    };
    let builtins = [Builtin {
        manifest: &BAD,
        make: BUILTINS[0].make,
    }];
    let err = CollectorRegistry::build(&builtins, &AppConfig::default(), None, &no_env).err();
    assert_eq!(err, Some(ManifestError::BadSettle("wow-ledgerlink", 0)));
}

#[test]
fn roots_resolve_home_and_env_vars_and_drop_unset_ones() {
    let env = |k: &str| match k {
        "HOME" => Some("/home/ann".to_owned()),
        "LOCALAPPDATA" => Some(r"C:\Users\ann\AppData\Local".to_owned()),
        _ => None,
    };
    assert_eq!(
        resolve_root("~/Documents/Larian", &env),
        Some(PathBuf::from("/home/ann/Documents/Larian"))
    );
    assert_eq!(
        resolve_root(r"%LOCALAPPDATA%\Larian Studios", &env),
        Some(PathBuf::from(r"C:\Users\ann\AppData\Local\Larian Studios"))
    );
    assert_eq!(resolve_root("%NOPE%/x", &env), None);
    assert_eq!(resolve_root("%UNCLOSED/x", &env), None);
}

#[test]
fn file_patterns_match_any_case_with_wildcards() {
    assert!(glob_match("LedgerLink.lua", "ledgerlink.LUA"));
    assert!(!glob_match("LedgerLink.lua", "LedgerLink.lua.bak"));
    assert!(glob_match("*.lsv", "QuickSave_12.LSV"));
    assert!(glob_match("a*b*c", "aXXbYYc"));
    assert!(!glob_match("*.lsv", "save.lsv.tmp"));
    assert!(
        !glob_match("ab*ba", "aba"),
        "prefix and suffix may not overlap"
    );
    assert!(WOW_LEDGERLINK.matches_file("LEDGERLINK.lua"));
}

#[test]
fn default_config_registers_wow_disabled_with_the_wow_dir() {
    // Overrides must be absolute on the host running the test.
    let dir = if cfg!(windows) {
        r"D:\Games\WoW"
    } else {
        "/games/wow"
    };
    let cfg = AppConfig {
        wow_dir: dir.into(),
        ..AppConfig::default()
    };
    let reg = build(&cfg);
    let ids: Vec<_> = reg.entries().iter().map(|e| e.manifest.id).collect();
    assert_eq!(ids, vec!["wow-ledgerlink", "bg3-save", "bg3-screenshot"]);
    let wow = reg.get("wow-ledgerlink").expect("wow registered");
    assert!(!wow.enabled, "opt-in: off until the user turns it on");
    assert_eq!(wow.roots, vec![PathBuf::from(dir)]);
    assert!(!reg.get("bg3-save").expect("bg3").enabled, "BG3 is opt-in");
    let shots = reg.get("bg3-screenshot").expect("bg3 screenshots");
    assert!(!shots.enabled, "BG3 screenshots are opt-in");
    assert!(reg.notes.is_empty());
}

#[test]
fn enabled_map_switches_a_collector_and_unknown_ids_are_noted_not_fatal() {
    let mut cfg = AppConfig::default();
    cfg.set_collector_enabled("wow-ledgerlink", true);
    cfg.set_collector_enabled("eve-online", true);
    let reg = build(&cfg);
    assert!(reg.get("wow-ledgerlink").expect("wow").enabled);
    assert!(reg.get("eve-online").is_none());
    assert_eq!(
        reg.notes,
        vec![r#"Unknown collector "eve-online" in settings; ignored"#.to_owned()]
    );
    cfg.set_collector_enabled("wow-ledgerlink", false);
    assert!(!build(&cfg).get("wow-ledgerlink").expect("wow").enabled);
}

#[test]
fn snapshot_is_a_read_only_view_of_each_collector() {
    let dir = if cfg!(windows) {
        r"D:\games\wow"
    } else {
        "/games/wow"
    };
    let mut cfg = AppConfig {
        wow_dir: dir.into(),
        ..AppConfig::default()
    };
    cfg.set_collector_enabled("wow-ledgerlink", true);
    let snap = build(&cfg).snapshot();
    let json = serde_json::to_value(&snap).expect("serialises");
    assert_eq!(
        json,
        serde_json::json!([{
            "id": "wow-ledgerlink",
            "displayName": "LedgerLink",
            "contractId": "wow-ledgerlink",
            "contractVersion": 1,
            "enabled": true,
            "roots": [dir],
            "filePatterns": ["LedgerLink.lua"],
            "settleMs": 3000
        }, {
            "id": "bg3-save",
            "displayName": "Baldur's Gate 3",
            "contractId": "bg3-save",
            "contractVersion": 1,
            "enabled": false,
            "roots": [],
            "filePatterns": ["*.lsv"],
            "settleMs": 3000
        }, {
            "id": "bg3-screenshot",
            "displayName": "BG3 screenshots",
            "contractId": "bg3-screenshot",
            "contractVersion": 1,
            "enabled": false,
            "roots": [r"C:\Program Files (x86)\Steam\userdata"],
            "filePatterns": ["*.jpg", "*.png"],
            "settleMs": 2000
        }])
    );
}

#[test]
fn legacy_ledgerlink_flag_migrates_and_is_not_written_back() {
    let dir = temp_dir("cfg-migrate");
    let file = dir.join("config.json");
    std::fs::write(
        &file,
        r#"{"serverUrl":"https://raid.gamernight.net","wowDir":"D:\\WoW","ledgerlinkEnabled":true}"#,
    )
    .expect("write legacy config");
    let cfg = AppConfig::load(&file);
    assert!(
        cfg.collector_enabled("wow-ledgerlink", false),
        "legacy on → on"
    );
    assert!(build(&cfg).get("wow-ledgerlink").expect("wow").enabled);
    cfg.save(&file).expect("save");
    let raw = std::fs::read_to_string(&file).expect("read");
    assert!(
        !raw.contains("ledgerlinkEnabled"),
        "legacy key dropped: {raw}"
    );
    assert!(
        raw.contains(r#""wow-ledgerlink": true"#),
        "new map written: {raw}"
    );
    assert_eq!(AppConfig::load(&file), cfg, "round-trips after migration");

    // An explicit per-collector entry beats a stale legacy flag.
    std::fs::write(
        &file,
        r#"{"ledgerlinkEnabled":true,"collectorsEnabled":{"wow-ledgerlink":false}}"#,
    )
    .expect("write mixed config");
    assert!(!AppConfig::load(&file).collector_enabled("wow-ledgerlink", true));
    // Legacy off (or absent) stays off.
    std::fs::write(&file, r#"{"ledgerlinkEnabled":false}"#).expect("write");
    assert!(!AppConfig::load(&file).collector_enabled("wow-ledgerlink", false));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_registry_drives_the_wow_collector_end_to_end() {
    let root = temp_dir("registry-wow");
    let file = install(&root, "12345678#1", MULTI);
    let mut cfg = AppConfig {
        wow_dir: root.to_string_lossy().into_owned(),
        ..AppConfig::default()
    };
    cfg.set_collector_enabled("wow-ledgerlink", true);
    let reg = build(&cfg);
    assert_eq!(
        reg.watch_roots(),
        vec![root.join("_classic_era_").join("WTF")]
    );
    let wanting: Vec<_> = reg.wanting(&file).map(|e| e.manifest.id).collect();
    assert_eq!(
        wanting,
        vec!["wow-ledgerlink"],
        "a LedgerLink.lua change settles"
    );
    assert_eq!(reg.wanting(&file.with_file_name("Details.lua")).count(), 0);
    let wow = reg.get("wow-ledgerlink").expect("wow");
    let scan = wow
        .collector
        .scan("https://raid.example.net", &SentState::default());
    assert_eq!(scan.files, 1);
    assert!(
        !scan.pending.is_empty(),
        "exports found through the registry"
    );
    let _ = std::fs::remove_dir_all(root);
}
