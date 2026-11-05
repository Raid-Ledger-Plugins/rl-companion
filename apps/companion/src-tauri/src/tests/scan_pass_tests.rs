//! A collector pass stamps `/dev/v1/collectors` when it ends, and its pending
//! count includes items the collector found but could not queue yet.

use crate::collector::{Collector, PendingUpload, Scan, SentState};
use crate::config::AppConfig;
use crate::manifest::{CollectorManifest, Platform, WOW_LEDGERLINK};
use crate::registry::{Builtin, CollectorRegistry};
use crate::runtime::Runtime;
use crate::scan_report::ScanPass;
use crate::scope::WatchScope;
use serde_json::json;
use std::path::PathBuf;

const ID: &str = "fake-pass";

struct Fake;

impl Collector for Fake {
    fn id(&self) -> &'static str {
        ID
    }
    fn watch_roots(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    /// One queued export and two items waiting for a hub.
    fn scan(&self, _: &str, _: &SentState) -> Scan {
        let queued = PendingUpload {
            campaign: None,
            contract: ID,
            scope: "https://rl.test|/fake".into(),
            item: "i1".into(),
            sha256: "aa".into(),
            label: "Thrall".into(),
            body: json!({}),
            blobs: vec![],
        };
        Scan {
            pending: vec![queued],
            waiting: 2,
            ..Scan::default()
        }
    }
}

fn make(_: &[PathBuf], _: WatchScope, _: &AppConfig) -> Box<dyn Collector> {
    Box::new(Fake)
}

const MANIFEST: CollectorManifest = CollectorManifest {
    id: ID,
    display_name: "Fake",
    contract_id: ID,
    default_roots: &[],
    ..WOW_LEDGERLINK
};

const BUILTINS: &[Builtin] = &[Builtin {
    manifest: &MANIFEST,
    make,
}];

#[test]
fn a_pass_stamps_its_scan_when_it_ends_counting_hub_waiting_items() {
    let rt = Runtime::new(super::temp_dir("scan-pass").join("sent.json"));
    let mut cfg = AppConfig::default();
    cfg.set_collector_enabled(ID, true);
    let reg = CollectorRegistry::build(BUILTINS, &cfg, Some(Platform::Linux), &|_| None)
        .expect("fake manifest is valid");
    let entry = reg.get(ID).expect("fake entry");
    {
        let pass = ScanPass::begin(&rt, entry, "https://rl.test").expect("switched on");
        assert_eq!(pass.scan.pending.len(), 1);
        let early = rt.snapshot(&cfg).scan.get(ID).cloned();
        assert_eq!(early, None, "stamped only once the pass ends");
    }
    let report = rt.snapshot(&cfg).scan.get(ID).cloned().unwrap_or_default();
    assert!(
        report.last_scan_at.is_some(),
        "the pass stamped no scan: {report:?}"
    );
    assert_eq!(
        report.pending, 3,
        "1 queued + 2 waiting for a hub: {report:?}"
    );
}

#[test]
fn a_switched_off_collector_starts_no_pass_and_forgets_its_scan() {
    let rt = Runtime::new(super::temp_dir("scan-pass-off").join("sent.json"));
    rt.scanned(ID, &[], 4);
    let cfg = AppConfig::default();
    let reg = CollectorRegistry::build(BUILTINS, &cfg, Some(Platform::Linux), &|_| None)
        .expect("fake manifest is valid");
    let entry = reg.get(ID).expect("fake entry");
    assert!(ScanPass::begin(&rt, entry, "https://rl.test").is_none());
    assert_eq!(
        rt.snapshot(&cfg).scan.get(ID),
        None,
        "an off collector reports no scan"
    );
}
