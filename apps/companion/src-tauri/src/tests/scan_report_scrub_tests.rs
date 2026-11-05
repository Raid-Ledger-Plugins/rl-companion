//! The dev-bridge path scrub is fail-closed: from where an absolute path
//! starts, the rest of the line is `<path>`, unless the path opened right
//! after a quote or bracket, which then ends it.

use crate::scan_report::scrub_paths;

fn check(cases: &[(&str, &str)]) {
    for (raw, want) in cases {
        assert_eq!(scrub_paths(raw), *want, "scrubbing {raw:?}");
    }
}

#[test]
fn an_apostrophe_never_ends_a_path() {
    check(&[
        (
            r"Cannot watch C:\Users\O'Brien\AppData\Local\Larian Studios\Baldur's Gate 3\PlayerProfiles (os error 5)",
            "Cannot watch <path>",
        ),
        (
            r"open 'C:\Users\O'Brien\Baldur's Gate 3\Tav.lsv' failed",
            "open '<path>' failed",
        ),
        (
            r#"read "C:\Users\O'Brien\Baldur's Gate 3\Tav.lsv" (denied)"#,
            r#"read "<path>" (denied)"#,
        ),
        ("open '/home/o'brien/x", "open '<path>"),
        (r"cannot read \Users\O'Brien\x", "cannot read <path>"),
    ]);
}

#[test]
fn quoted_and_bracketed_paths_end_at_their_closer() {
    check(&[
        (
            r#"Could not read "C:\Program Files (x86)\Steam\userdata\9\x.lua" (denied)"#,
            r#"Could not read "<path>" (denied)"#,
        ),
        (
            r"failed (C:\Program Files (x86)\Steam\x) retry",
            "failed (<path>) retry",
        ),
        (
            r#"No path was found. about ["D:\\Games\\World of Warcraft\\WTF"]"#,
            r#"No path was found. about ["<path>"]"#,
        ),
        ("paths [/tmp/a b/c, /srv/x]", "paths [<path>]"),
        ("in [/tmp/x/y]", "in [<path>]"),
        ("open '/mnt/a b/c' failed", "open '<path>' failed"),
        (
            r#"io {:?} "D:\\Games\\World of Warcraft\\WTF""#,
            r#"io {:?} "<path>""#,
        ),
        (r"got <C:\Users\O'Brien\x> back", "got <<path>> back"),
    ]);
}

#[test]
fn a_bare_path_takes_the_rest_of_its_line() {
    check(&[
        (
            r"Cannot watch C:\Program Files (x86)\Steam\steamapps\common (denied)",
            "Cannot watch <path>",
        ),
        (
            r"Cannot watch D:\Games\World of Warcraft\WTF (No path was found. about [x])",
            "Cannot watch <path>",
        ),
        (
            r"cannot read \\nas\games\WoW\WTF\x.lua (denied)",
            "cannot read <path>",
        ),
        (
            r"cannot read \\?\C:\Games\BG3\save.lsv",
            "cannot read <path>",
        ),
        (
            r"cannot read D:/Games/World of Warcraft/WTF",
            "cannot read <path>",
        ),
        (
            "cannot read ~/Library/Application Support/x/y.lua",
            "cannot read <path>",
        ),
        ("cannot watch /home/tester/.steam/x", "cannot watch <path>"),
        (
            "Acct: cannot read /mnt/wow/WTF/x.lua (denied)",
            "Acct: cannot read <path>",
        ),
        ("open file:///home/x y", "open <path>"),
        ("a /tmp/x y\nnext line ok\r\n", "a <path>\nnext line ok\r\n"),
    ]);
}

#[test]
fn a_path_after_a_separator_is_caught() {
    check(&[
        ("a|/tmp/x y|b", "a|<path>"),
        ("error:/home/o'brien/x lsv", "error:<path>"),
        ("path=~/Library/Application Support/x tail", "path=<path>"),
        (r"at <D:\x y", "at <<path>"),
        (r"k={C:\a b\c}", "k={<path>}"),
        (r"list a,C:\x;y", "list a,<path>"),
    ]);
}

#[test]
fn text_that_is_not_a_path_is_kept() {
    for kept in [
        "LedgerLink uploads turned on/off",
        "Rejected 422: bad export",
        "5 / 10 sent",
        "Server https://rl.test/api answered",
        "Could not save (disk full)",
        "it's Larian's: ok",
        "ratio 3:4, a=b",
    ] {
        assert_eq!(scrub_paths(kept), kept);
    }
}
