//! Activity lines reach the `/dev/v1/logs` ring without names or paths: the
//! subject is passed apart, never parsed back out, and every absolute path —
//! quoted, bare, with spaces or parentheses — is `<path>` to its end.

use crate::config::AppConfig;
use crate::dev_bridge::logbuf;
use crate::manifest::{resolve_override, BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::Runtime;
use crate::scan_report::{note_body, scrub_paths};
use serde_json::Value;
use std::path::PathBuf;

const WOW: &str = WOW_LEDGERLINK.id;

fn ring_line(marker: &str) -> Value {
    logbuf::global()
        .select(0, None, None)
        .into_iter()
        .map(|(_, v)| v)
        .find(|v| v["msg"].as_str().is_some_and(|m| m.contains(marker)))
        .unwrap_or_else(|| panic!("no ring line carries {marker}"))
}

fn window_has(rt: &Runtime, text: &str) -> bool {
    rt.snapshot(&AppConfig::default())
        .log
        .iter()
        .any(|l| l.text == text)
}

#[test]
fn a_subject_with_a_colon_never_reaches_the_ring() {
    let rt = Runtime::new(super::temp_dir("scan-log-subject").join("sent.json"));
    let name = "Act 2: The Shadow-Cursed Lands";
    rt.log_about(BG3_SAVE.id, name, "Uploaded marker_s11");
    let line = ring_line("marker_s11");
    assert_eq!(line["msg"], "Uploaded marker_s11", "{line}");
    assert_eq!(line["target"], "collector:bg3-save", "{line}");
    assert!(
        window_has(&rt, "Act 2: The Shadow-Cursed Lands: Uploaded marker_s11"),
        "the window keeps the name"
    );
}

#[test]
fn a_scan_note_drops_its_account_prefix_and_a_plain_line_stays_whole() {
    let rt = Runtime::new(super::temp_dir("scan-log-note").join("sent.json"));
    rt.log_note(WOW, "Thrall: Rejected 422: marker_q81");
    let line = ring_line("marker_q81");
    assert_eq!(line["msg"], "Rejected 422: marker_q81", "{line}");
    assert_eq!(line["target"], "collector:wow-ledgerlink", "{line}");
    assert!(window_has(&rt, "Thrall: Rejected 422: marker_q81"));
    rt.log_for(WOW, "LedgerLink uploads turned on marker_l07");
    let line = ring_line("marker_l07");
    assert_eq!(line["msg"], "LedgerLink uploads turned on marker_l07");
    assert_eq!(note_body("no prefix here"), "no prefix here");
}

#[test]
fn the_real_notify_watch_error_leaks_no_path() {
    let root = PathBuf::from(r"D:\Games\World of Warcraft\_forever_\WTF");
    let e = notify::Error::path_not_found().add_path(root.clone());
    // `watch.rs`'s note, verbatim shape.
    let note = format!("Cannot watch {} ({e}) marker_n93", root.display());
    assert!(note.contains(r#"about [""#), "notify shape changed: {note}");
    let rt = Runtime::new(super::temp_dir("scan-log-notify").join("sent.json"));
    rt.log(note);
    let msg = ring_line("marker_n93")["msg"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert_eq!(
        msg,
        r#"Cannot watch <path> (No path was found. about ["<path>"]) marker_n93"#
    );
}

#[test]
fn every_absolute_path_shape_is_scrubbed_to_its_end() {
    let cases = [
        (
            r#"Could not read "C:\Program Files (x86)\Steam\userdata\9\x.lua" (denied)"#,
            r#"Could not read "<path>" (denied)"#,
        ),
        (
            r"Cannot watch C:\Program Files (x86)\Steam\steamapps\common (denied)",
            "Cannot watch <path> (denied)",
        ),
        (
            r"cannot read \\nas\games\WoW\WTF\x.lua (denied)",
            "cannot read <path> (denied)",
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
            "Acct: cannot read <path> (denied)",
        ),
        // Two bare paths on one line merge into one `<path>`: over-redaction,
        // never a leaked tail.
        ("paths [/tmp/a b/c, /srv/x]", "paths [<path>]"),
        ("in [/tmp/x/y]", "in [<path>]"),
        ("open '/mnt/a b/c' failed", "open '<path>' failed"),
        (
            r#"io {:?} "D:\\Games\\World of Warcraft\\WTF""#,
            r#"io {:?} "<path>""#,
        ),
    ];
    for (raw, want) in cases {
        assert_eq!(scrub_paths(raw), want, "scrubbing {raw:?}");
    }
}

#[test]
fn text_that_is_not_a_path_is_kept() {
    for kept in [
        "LedgerLink uploads turned on/off",
        "Rejected 422: bad export",
        "5 / 10 sent",
        "Server https://rl.test/api answered",
        "Could not save (disk full)",
    ] {
        assert_eq!(scrub_paths(kept), kept);
    }
}

#[test]
fn a_refused_folder_override_does_not_echo_the_folder() {
    let err = resolve_override("saves/Thrall", &|_| None).unwrap_err();
    assert!(err.contains("must be a full path"), "{err}");
    assert!(!err.contains("Thrall"), "the folder leaked: {err}");
}
