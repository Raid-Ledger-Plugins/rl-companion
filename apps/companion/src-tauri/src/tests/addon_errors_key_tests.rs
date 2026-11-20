//! ROK-2099 S5 review D1: `accountKey` is HMAC-SHA256 of the account folder
//! keyed with a per-install secret kept only on this machine.

use crate::addon_errors_collector::AddonErrorsCollector;
use crate::addon_errors_key::{load_or_create_with, KEY_FILE};
use crate::collector::{Collector, SentState};
use rl_collectors::{hmac_sha256_hex, sha256_hex};

const LUA: &str = "BugGrabberDB = {\n[\"errors\"] = {\n{ [\"message\"] = \"Interface\\\\AddOns\\\\Foo\\\\F.lua:1: boom\", [\"counter\"] = 1 },\n},\n}\n";

fn wow_root(name: &str) -> std::path::PathBuf {
    let root = super::temp_dir(name);
    let sv = root.join("_classic_era_/WTF/Account/ACCT1/SavedVariables");
    std::fs::create_dir_all(&sv).expect("sv dir");
    std::fs::write(sv.join("!BugGrabber.lua"), LUA).expect("write sv");
    root
}

fn account_key(root: &std::path::Path, key_dir: &std::path::Path) -> String {
    let c = AddonErrorsCollector::new(root)
        .with_utc_offset(0)
        .with_key_dir(key_dir);
    let scan = c.scan("https://raid.example", &SentState::default());
    let body = &scan.pending.first().expect("one batch").body;
    body.pointer("/payload/accountKey")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("no accountKey in {body}"))
        .to_owned()
}

fn stored_key(dir: &std::path::Path) -> [u8; 32] {
    load_or_create_with(dir, |_| panic!("the key file must already exist")).expect("stored")
}

#[test]
fn the_uploaded_account_key_is_the_install_keyed_hmac_of_the_folder() {
    let root = wow_root("aek-upload");
    let keys = super::temp_dir("aek-upload-keys");
    let sent = account_key(&root, &keys);
    let key = stored_key(&keys);
    assert_eq!(sent, hmac_sha256_hex(&key, b"ACCT1"));
    assert_ne!(
        sent,
        sha256_hex(b"ACCT1"),
        "never the bare, reversible hash"
    );
    assert_eq!(
        account_key(&root, &keys),
        sent,
        "same install => same value"
    );
    let other = super::temp_dir("aek-upload-other");
    assert_ne!(
        account_key(&root, &other),
        sent,
        "another install => another value"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(keys.join(KEY_FILE))
            .expect("key file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let body = std::fs::read_to_string(keys.join(KEY_FILE)).expect("key file");
    assert!(!sent.contains(body.trim()), "the key itself is never sent");
}

#[test]
fn the_key_file_is_created_once_and_reused() {
    let dir = super::temp_dir("aek-once");
    let first = load_or_create_with(&dir, |k| {
        k.fill(0xab);
        true
    });
    assert_eq!(first, Some([0xab; 32]));
    assert_eq!(
        std::fs::read_to_string(dir.join(KEY_FILE)).expect("file"),
        "ab".repeat(32)
    );
    assert_eq!(stored_key(&dir), [0xab; 32]);
}

#[test]
fn a_missing_or_corrupt_key_file_is_regenerated_without_panicking() {
    let dir = super::temp_dir("aek-corrupt");
    for bad in ["", "zz", &"g".repeat(64), &"ab".repeat(31), &"é".repeat(32)] {
        std::fs::write(dir.join(KEY_FILE), bad).expect("write bad");
        let key = load_or_create_with(&dir, |k| {
            k.fill(0x11);
            true
        });
        assert_eq!(key, Some([0x11; 32]), "{bad:?}");
        assert_eq!(stored_key(&dir), [0x11; 32]);
    }
    std::fs::remove_file(dir.join(KEY_FILE)).expect("rm");
    assert_eq!(
        load_or_create_with(&dir, |_| false),
        None,
        "no RNG => no key, nothing sent"
    );
}

#[test]
fn a_missing_key_file_is_created() {
    let dir = super::temp_dir("aek-missing");
    assert!(!dir.join(KEY_FILE).exists());
    let key = load_or_create_with(&dir, |k| {
        k.fill(0x22);
        true
    });
    assert_eq!(key, Some([0x22; 32]));
    assert_eq!(stored_key(&dir), [0x22; 32]);
}

#[test]
fn garbage_key_content_is_replaced() {
    let dir = super::temp_dir("aek-garbage");
    std::fs::write(dir.join(KEY_FILE), [0xff, 0xfe, 0x00, b'x']).expect("write garbage");
    let key = load_or_create_with(&dir, |k| {
        k.fill(0x33);
        true
    });
    assert_eq!(key, Some([0x33; 32]));
    assert_eq!(stored_key(&dir), [0x33; 32]);
}

#[test]
fn an_unreadable_key_file_is_never_overwritten_and_nothing_is_sent() {
    // A directory at the key path: opening/reading it fails with an error that
    // is neither "not found" nor bad content (stands in for a locked file).
    let dir = super::temp_dir("aek-unreadable");
    let path = dir.join(KEY_FILE);
    std::fs::create_dir_all(&path).expect("dir at key path");
    std::fs::write(path.join("marker"), "keep").expect("marker");
    let key = load_or_create_with(&dir, |_| panic!("must not generate a new key"));
    assert_eq!(key, None, "no key => nothing uploaded this cycle");
    assert!(path.is_dir(), "the existing entry is left alone");
    assert_eq!(
        std::fs::read_to_string(path.join("marker")).expect("marker kept"),
        "keep"
    );
}
