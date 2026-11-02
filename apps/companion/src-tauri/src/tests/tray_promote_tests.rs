use crate::tray_promote::{find_entry, normalise_path, paths_match, should_promote};

const EXE: &str = r"C:\Users\Op\AppData\Local\RL Companion\rl-companion.exe";
const LOCAL_APP_DATA: &str = "{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}";
const PROGRAM_FILES: &str = "{6D809377-6AF0-444B-8957-A3773F02200E}";

fn folders() -> Vec<(String, String)> {
    let kf = |id: &str, dir: &str| (id.to_owned(), dir.to_owned());
    vec![
        kf(LOCAL_APP_DATA, r"C:\Users\Op\AppData\Local"),
        kf(PROGRAM_FILES, r"C:\Program Files\"),
    ]
}

#[test]
fn promotion_runs_only_until_it_succeeded_once() {
    assert!(should_promote(false));
    assert!(!should_promote(true));
}

#[test]
fn paths_compare_case_and_separator_insensitively() {
    let f = folders();
    let lower = r"c:\users\op\appdata\local\rl companion\RL-COMPANION.EXE";
    let verbatim = r"\\?\C:/Users/Op/AppData/Local/RL Companion/rl-companion.exe";
    assert!(paths_match(lower, EXE, &f));
    assert!(paths_match(verbatim, EXE, &f));
    assert!(!paths_match(r"C:\Other\rl-companion.exe", EXE, &f));
    assert!(!paths_match("", "", &f));
}

#[test]
fn known_folder_ids_are_expanded() {
    let f = folders();
    let local = r"{f1b32785-6fba-4fcf-9d55-7b8e7f157091}\RL Companion\rl-companion.exe";
    assert!(paths_match(local, EXE, &f));
    let pf = format!(r"{PROGRAM_FILES}\RL Companion\rl-companion.exe");
    let want = r"c:\program files\rl companion\rl-companion.exe";
    assert_eq!(normalise_path(&pf, &f), want);
    // A non-ASCII path shorter than an id must not panic on a char boundary.
    assert_eq!(normalise_path("Ä", &f), "ä");
}

#[test]
fn the_entry_for_our_exe_is_found_among_others() {
    let f = folders();
    let entries = vec![
        ("111".to_owned(), r"C:\Windows\explorer.exe".to_owned()),
        ("222".to_owned(), EXE.to_uppercase()),
    ];
    assert_eq!(find_entry(&entries, EXE, &f), Some("222"));
    assert_eq!(find_entry(&entries[..1], EXE, &f), None);
}
