//! ROK-2061: a collector only ever reads under its resolved roots.

use super::collector_tests::{install, MULTI};
use super::temp_dir;
use crate::collector::{Collector, SentState};
use crate::scope::{ScopeError, WatchScope};
use crate::wow_collector::LedgerLinkCollector;

#[test]
fn paths_inside_the_root_resolve_and_paths_outside_are_refused() {
    let base = temp_dir("scope-basic");
    let root = base.join("root");
    let outside = base.join("outside.txt");
    std::fs::create_dir_all(root.join("sub")).expect("mkdir");
    std::fs::write(root.join("sub").join("in.txt"), "in").expect("write in");
    std::fs::write(&outside, "secret").expect("write out");
    let scope = WatchScope::new(std::slice::from_ref(&root));

    assert_eq!(
        scope
            .read(&root.join("sub").join("in.txt"))
            .expect("inside"),
        b"in"
    );
    assert!(scope.contains(&root), "the root itself is in scope");
    assert_eq!(
        scope.check(&outside),
        Err(ScopeError::Outside(outside.clone()))
    );
    let dotdot = root.join("sub").join("..").join("..").join("outside.txt");
    assert_eq!(
        scope.check(&dotdot),
        Err(ScopeError::Outside(dotdot.clone())),
        "`..` traversal out of the root is refused"
    );
    assert!(scope.read(&dotdot).is_err());
    let missing = root.join("nope");
    assert_eq!(
        scope.check(&missing),
        Err(ScopeError::Unresolvable(missing))
    );
    assert!(
        !WatchScope::default().contains(&root),
        "an empty scope grants nothing"
    );
    let _ = std::fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_root_is_refused() {
    let base = temp_dir("scope-symlink");
    let root = base.join("root");
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&root).expect("mkdir root");
    std::fs::create_dir_all(&elsewhere).expect("mkdir elsewhere");
    std::fs::write(elsewhere.join("secret.txt"), "secret").expect("write");
    std::os::unix::fs::symlink(&elsewhere, root.join("link")).expect("symlink");
    let scope = WatchScope::new(std::slice::from_ref(&root));
    let escaped = root.join("link").join("secret.txt");
    assert_eq!(
        scope.check(&escaped),
        Err(ScopeError::Outside(escaped.clone())),
        "symlink escape must be refused"
    );
    assert!(scope.read(&escaped).is_err());
    assert!(scope.read_dir(&root.join("link")).is_err());
    let _ = std::fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn the_wow_collector_never_reads_a_symlinked_file_outside_the_wow_folder() {
    let base = temp_dir("scope-wow");
    let root = base.join("wow");
    let real = install(&root, "12345678#1", MULTI);
    // Move the real file out of the WoW folder and leave a symlink behind.
    let outside = base.join("LedgerLink.lua");
    std::fs::rename(&real, &outside).expect("move out");
    std::os::unix::fs::symlink(&outside, &real).expect("symlink");
    let scan =
        LedgerLinkCollector::new(&root).scan("https://raid.example.net", &SentState::default());
    assert_eq!(
        (scan.files, scan.pending.len()),
        (0, 0),
        "the escaped file is not read: {:?}",
        scan.notes
    );

    // A symlinked flavor folder pointing outside is not even listed.
    let base2 = temp_dir("scope-wow-flavor");
    let root2 = base2.join("wow");
    let other = base2.join("other-install");
    install(&other, "12345678#1", MULTI);
    std::fs::create_dir_all(&root2).expect("mkdir");
    std::os::unix::fs::symlink(other.join("_classic_era_"), root2.join("_classic_era_"))
        .expect("symlink flavor");
    let collector = LedgerLinkCollector::new(&root2);
    assert!(
        collector.watch_roots().is_empty(),
        "no WTF outside the root is watched"
    );
    assert_eq!(
        collector
            .scan("https://raid.example.net", &SentState::default())
            .files,
        0
    );
    let _ = std::fs::remove_dir_all(base);
    let _ = std::fs::remove_dir_all(base2);
}
