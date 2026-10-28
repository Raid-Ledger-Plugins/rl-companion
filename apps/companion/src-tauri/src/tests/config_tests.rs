use crate::config::{normalise_server_url, sanitise_device_name, AppConfig, DEFAULT_SERVER_URL};

#[test]
fn server_url_is_reduced_to_an_origin() {
    assert_eq!(
        normalise_server_url("https://raid.gamernight.net/").as_deref(),
        Ok("https://raid.gamernight.net")
    );
    assert_eq!(
        normalise_server_url("  https://slot-2.gamernight.net ").as_deref(),
        Ok("https://slot-2.gamernight.net")
    );
    assert_eq!(
        normalise_server_url("http://localhost:5173").as_deref(),
        Ok("http://localhost:5173")
    );
}

#[test]
fn server_url_rejects_unsafe_or_ambiguous_input() {
    for bad in [
        "http://raid.gamernight.net",
        "ftp://x.y",
        "https://u:p@raid.gamernight.net",
        "https://raid.gamernight.net/api",
        "https://raid.gamernight.net/?a=1",
        "not a url",
    ] {
        assert!(
            normalise_server_url(bad).is_err(),
            "{bad} should be rejected"
        );
    }
}

#[test]
fn device_name_is_display_safe() {
    assert_eq!(
        sanitise_device_name("  Gaming PC  ").as_deref(),
        Some("Gaming PC")
    );
    assert_eq!(
        sanitise_device_name("Evil\u{202E}PC\n").as_deref(),
        Some("EvilPC")
    );
    assert_eq!(sanitise_device_name(" \t ").as_deref(), None);
    assert_eq!(
        sanitise_device_name(&"x".repeat(100)).map(|s| s.chars().count()),
        Some(64)
    );
}

#[test]
fn config_round_trips_and_defaults_to_prod() {
    let dir = std::env::temp_dir().join(format!("rlc-cfg-{}", std::process::id()));
    let file = dir.join("config.json");
    assert_eq!(
        AppConfig::load(&file).server_url,
        DEFAULT_SERVER_URL,
        "missing file → prod default"
    );
    let cfg = AppConfig {
        server_url: "https://slot-1.gamernight.net".into(),
        device_name: Some("PC".into()),
        wow_dir: r"D:\Games\WoW".into(),
        ledgerlink_enabled: true,
    };
    cfg.save(&file).expect("save");
    assert_eq!(AppConfig::load(&file), cfg);
    let raw = std::fs::read_to_string(&file).expect("read");
    assert!(!raw.contains("rlcd_"), "config never holds a token");
    let _ = std::fs::remove_dir_all(dir);
}
