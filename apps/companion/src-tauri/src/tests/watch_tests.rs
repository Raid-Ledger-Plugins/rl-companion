use super::temp_dir;
use crate::watch::{sample, SettleSet};
use crate::wow_collector::is_ledgerlink_file;
use std::path::Path;

#[test]
fn settle_set_reports_a_written_file_once_after_stable_samples() {
    let dir = temp_dir("settle");
    let file = dir.join("LedgerLink.lua");
    std::fs::write(&file, "LedgerLinkDB = {}\n").expect("write");
    let mut set = SettleSet::default();
    set.touch(file.clone());
    set.touch(file.clone()); // a second event for the same path is one entry
    assert!(set.tick().is_empty(), "first sample only arms the watcher");
    assert!(
        set.tick().is_empty(),
        "two equal samples are not yet settled"
    );
    assert_eq!(set.tick(), vec![file.clone()], "third equal sample settles");
    assert!(set.is_empty(), "a settled path is forgotten");
    assert!(set.tick().is_empty(), "and never reported twice");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn settle_set_rearms_while_the_file_keeps_changing() {
    let dir = temp_dir("settle-change");
    let file = dir.join("LedgerLink.lua");
    std::fs::write(&file, "a").expect("write");
    let mut set = SettleSet::default();
    set.touch(file.clone());
    assert!(set.tick().is_empty());
    assert!(set.tick().is_empty());
    std::fs::write(&file, "a longer body").expect("rewrite");
    assert!(set.tick().is_empty(), "a size change re-arms");
    assert!(set.tick().is_empty());
    assert_eq!(set.tick(), vec![file.clone()]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn missing_files_never_settle() {
    let dir = temp_dir("settle-missing");
    let file = dir.join("LedgerLink.lua");
    assert_eq!(sample(&file), None);
    let mut set = SettleSet::default();
    set.touch(file);
    for _ in 0..5 {
        assert!(set.tick().is_empty());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_ledgerlink_lua_is_relevant() {
    assert!(is_ledgerlink_file(Path::new(
        "x/SavedVariables/LedgerLink.lua"
    )));
    assert!(is_ledgerlink_file(Path::new(
        "x/SavedVariables/ledgerlink.LUA"
    )));
    assert!(!is_ledgerlink_file(Path::new(
        "x/SavedVariables/LedgerLink.lua.bak"
    )));
    assert!(!is_ledgerlink_file(Path::new(
        "x/SavedVariables/Details.lua"
    )));
}
