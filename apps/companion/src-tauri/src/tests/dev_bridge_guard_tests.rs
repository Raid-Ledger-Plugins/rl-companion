//! Write guard vs symlinks (ROK-2055 C2 review, Codex P2 + NIT-11): the
//! resolved parent must BE LedgerLink's folder (ROK-2191), nothing is written
//! through a symlinked step, and a link planted at the temp name is not followed.
#![cfg(unix)]

use crate::dev_bridge::write_guard::{guarded_write, WriteGuardError};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

const FLAVOR: &str = "_anniversary_";

fn addons(wow: &Path) -> PathBuf {
    wow.join(FLAVOR).join("Interface").join("AddOns")
}

fn queue(wow: &Path) -> PathBuf {
    addons(wow).join("LedgerLink").join("DevQueue.lua")
}

#[test]
fn writes_into_an_installed_ledgerlink_folder_when_nothing_is_linked() {
    let wow = super::temp_dir("guard-plain");
    std::fs::create_dir_all(addons(&wow).join("LedgerLink")).expect("mkdir");
    guarded_write(&wow, &queue(&wow), b"Q").expect("plain write");
    assert_eq!(std::fs::read(queue(&wow)).expect("read"), b"Q");
    let _ = std::fs::remove_dir_all(wow);
}

#[test]
fn a_ledgerlink_folder_linked_to_another_addon_inside_wow_is_refused() {
    let wow = super::temp_dir("guard-ll-link");
    let other = addons(&wow).join("SomeOtherAddon");
    std::fs::create_dir_all(&other).expect("mkdir");
    symlink(&other, addons(&wow).join("LedgerLink")).expect("link");
    let got = guarded_write(&wow, &queue(&wow), b"Q");
    assert!(
        matches!(got, Err(WriteGuardError::Outside(_))),
        "symlinked LedgerLink → another addon must be refused, got {got:?}"
    );
    assert!(
        !other.join("DevQueue.lua").exists(),
        "nothing written into the other addon through the link"
    );
    let _ = std::fs::remove_dir_all(wow);
}

#[test]
fn nothing_is_written_through_a_symlinked_ancestor() {
    let wow = super::temp_dir("guard-ancestor-link");
    let outside = super::temp_dir("guard-ancestor-outside");
    std::fs::create_dir_all(wow.join(FLAVOR).join("Interface")).expect("mkdir");
    std::fs::create_dir_all(outside.join("LedgerLink")).expect("mkdir outside");
    symlink(&outside, addons(&wow)).expect("link AddOns → outside");
    let got = guarded_write(&wow, &queue(&wow), b"Q");
    assert!(
        matches!(got, Err(WriteGuardError::Outside(_))),
        "a symlinked AddOns folder must be refused, got {got:?}"
    );
    assert!(
        !outside.join("LedgerLink").join("DevQueue.lua").exists(),
        "DevQueue.lua was written outside the WoW folder"
    );
    let _ = std::fs::remove_dir_all(wow);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn a_link_planted_at_the_temp_name_is_not_followed() {
    let wow = super::temp_dir("guard-tmp-link");
    let outside = super::temp_dir("guard-tmp-outside");
    let victim = outside.join("victim.txt");
    std::fs::write(&victim, b"keep").expect("victim");
    std::fs::create_dir_all(queue(&wow).parent().expect("parent")).expect("mkdir");
    let tmp = crate::config::tmp_path(&queue(&wow));
    symlink(&victim, &tmp).expect("plant tmp link");
    guarded_write(&wow, &queue(&wow), b"Q").expect("write");
    assert_eq!(
        std::fs::read(&victim).expect("read victim"),
        b"keep",
        "the planted temp link was followed and overwrote a file outside"
    );
    assert_eq!(std::fs::read(queue(&wow)).expect("read"), b"Q");
    assert!(
        std::fs::symlink_metadata(&tmp).is_err(),
        "temp name cleaned up"
    );
    let _ = std::fs::remove_dir_all(wow);
    let _ = std::fs::remove_dir_all(outside);
}
