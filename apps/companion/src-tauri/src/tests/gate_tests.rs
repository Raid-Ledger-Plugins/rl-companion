//! ROK-2061 review MAJOR 3: the confirm-first consent gate. No consent →
//! hold (nothing sent); consent → send the backlog.

use crate::collector::PendingUpload;
use crate::gate::{gate, Gate};
use crate::manifest::{BG3_SAVE, WOW_LEDGERLINK};
use crate::runtime::Runtime;
use serde_json::json;

fn pending(n: usize) -> Vec<PendingUpload> {
    (0..n)
        .map(|i| PendingUpload {
            campaign: None,
            contract: "bg3-save",
            scope: format!("https://rl.test|/saves/{i}.lsv"),
            item: "save".into(),
            sha256: format!("sha{i}"),
            label: format!("Save {i}"),
            body: json!({ "payload": { "saveName": format!("Save {i}"), "savedAt": format!("2026-10-3{i}") } }),
            blobs: vec![],
        })
        .collect()
}

#[test]
fn no_consent_holds_the_first_upload_and_sends_nothing() {
    let Gate::Hold(Some(held)) = gate(&BG3_SAVE, false, 2, &pending(2)) else {
        panic!("a confirm-first collector without consent must hold");
    };
    assert_eq!(held.keys.len(), 2, "the whole backlog is held");
}

#[test]
fn consent_sends_the_backlog() {
    assert_eq!(gate(&BG3_SAVE, true, 2, &pending(2)), Gate::Send);
}

#[test]
fn a_collector_without_confirm_first_sends_without_consent() {
    assert_eq!(gate(&WOW_LEDGERLINK, false, 1, &pending(1)), Gate::Send);
}

#[test]
fn no_files_and_nothing_waiting_never_hold_or_send() {
    for consented in [false, true] {
        assert_eq!(gate(&BG3_SAVE, consented, 0, &[]), Gate::NoFiles);
        assert_eq!(gate(&BG3_SAVE, consented, 3, &[]), Gate::UpToDate);
    }
}

#[test]
fn a_stale_hold_after_skip_is_not_re_shown() {
    // Review MINOR 5: a scan that started before Skip re-holds the same keys.
    let dir = super::temp_dir("gate-stale-hold");
    let rt = Runtime::new(dir.join("sent.json"));
    let batch = pending(2);
    let Gate::Hold(held) = gate(&BG3_SAVE, false, 2, &batch) else {
        panic!("held");
    };
    rt.set_held(BG3_SAVE.id, held.clone().map(|h| *h));
    assert_eq!(rt.skip_held(BG3_SAVE.id), 2);
    rt.set_held(BG3_SAVE.id, held.map(|h| *h));
    assert_eq!(
        rt.skip_held(BG3_SAVE.id),
        0,
        "already-skipped keys are not held again"
    );
}
