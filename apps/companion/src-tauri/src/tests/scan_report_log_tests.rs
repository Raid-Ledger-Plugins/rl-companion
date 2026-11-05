//! Activity lines reach the `/dev/v1/logs` ring without names or paths: the
//! subject is passed apart, never parsed back out. Path shapes:
//! `scan_report_scrub_tests.rs`.

use crate::collector::{Collector, Note, SentState};
use crate::config::AppConfig;
use crate::dev_bridge::logbuf;
use crate::manifest::{resolve_override, BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::Runtime;
use crate::scan_report::scrub_paths;
use crate::wow_collector::LedgerLinkCollector;
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
fn a_scan_note_subject_never_reaches_the_ring_even_with_a_colon() {
    let rt = Runtime::new(super::temp_dir("scan-log-note").join("sent.json"));
    let note = Note {
        subject: Some("Act 2: The Shadow: Tav.lsv".into()),
        text: "not readable yet marker_q81".into(),
    };
    rt.log_note(BG3_SAVE.id, &note);
    let line = ring_line("marker_q81");
    assert_eq!(line["msg"], "not readable yet marker_q81", "{line}");
    assert_eq!(line["target"], "collector:bg3-save", "{line}");
    assert!(window_has(
        &rt,
        "Act 2: The Shadow: Tav.lsv: not readable yet marker_q81"
    ));
    let plain = Note {
        subject: None,
        text: "BG3 screenshots: 2 waiting marker_l07".into(),
    };
    rt.log_note(WOW, &plain);
    let line = ring_line("marker_l07");
    assert_eq!(line["msg"], "BG3 screenshots: 2 waiting marker_l07");
    rt.log_for(WOW, "LedgerLink uploads turned on marker_l08");
    assert_eq!(
        ring_line("marker_l08")["msg"],
        "LedgerLink uploads turned on marker_l08"
    );
}

#[test]
fn a_skipped_outbox_key_stays_in_the_window() {
    let root = super::temp_dir("scan-log-skipped-key");
    let lua =
        "LedgerLinkDB = {\n[\"outbox\"] = {\n[\"marker_k44: C:\\\\Users\\\\x\"] = 5,\n},\n}\n";
    super::collector_tests::install(&root, "Thrall", lua);
    let scan =
        LedgerLinkCollector::new(&root).scan("https://raid.example.net", &SentState::default());
    let note = scan.notes.first().expect("one skipped note");
    assert_eq!(note.text, "skipped (NotATable)", "{:?}", scan.notes);
    let rt = Runtime::new(root.join("sent.json"));
    rt.log_note(WOW, note);
    let leaked: Vec<Value> = logbuf::global()
        .select(0, None, None)
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| v.to_string().contains("marker_k44"))
        .collect();
    assert!(
        leaked.is_empty(),
        "the outbox key reached the ring: {leaked:?}"
    );
    assert!(window_has(
        &rt,
        r"Thrall, outbox entry marker_k44: C:\Users\x: skipped (NotATable)"
    ));
}

#[test]
fn the_real_notify_watch_error_leaks_no_path() {
    let root = PathBuf::from(r"D:\Games\World of Warcraft\_forever_\WTF");
    let e = notify::Error::path_not_found().add_path(root.clone());
    // `watch.rs`'s note, verbatim shape (marker first: the rest is a path).
    let note = format!("marker_n93 Cannot watch {} ({e})", root.display());
    assert!(note.contains(r#"about [""#), "notify shape changed: {note}");
    let rt = Runtime::new(super::temp_dir("scan-log-notify").join("sent.json"));
    rt.log(note);
    let msg = ring_line("marker_n93")["msg"].clone();
    assert_eq!(msg, "marker_n93 Cannot watch <path>");
    // notify's own text alone keeps its words and loses the quoted path.
    assert_eq!(
        scrub_paths(&e.to_string()),
        r#"No path was found. about ["<path>"]"#
    );
}

#[test]
fn a_refused_folder_override_does_not_echo_the_folder() {
    let err = resolve_override("saves/Thrall", &|_| None).unwrap_err();
    assert!(err.contains("must be a full path"), "{err}");
    assert!(!err.contains("Thrall"), "the folder leaked: {err}");
}
