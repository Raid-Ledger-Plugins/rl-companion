//! ROK-2099 S5 part 2: the BugGrabber addon-errors collector in the app —
//! off by default (nothing read), state committed only once the server
//! settled a batch, the 50-record split wired through, and its registry +
//! `/dev/v1/collectors` row.

use super::dev_bridge_unit_tests::schema_errors;
use crate::addon_errors_collector::{record_items, refused_note, AddonErrorsCollector, CONTRACT};
use crate::collector::{Collector, SentState};
use crate::config::AppConfig;
use crate::dev_bridge::collectors::collectors_body;
use crate::manifest::{Platform, WOW_ADDON_ERRORS};
use crate::registry::{CollectorRegistry, BUILTINS};
use crate::runtime::{effect, Runtime};
use crate::scan_report::unsettled;
use crate::upload::UploadOutcome;
use std::path::{Path, PathBuf};

const ORIGIN: &str = "https://raid.example";
const FIXTURE: &str =
    include_str!("../../../../../crates/rl-collectors/tests/fixtures/addon_errors/buggrabber.lua");

/// A WoW folder with one account holding `lua` as `!BugGrabber.lua`.
fn wow_root(name: &str, lua: &str) -> PathBuf {
    let root = super::temp_dir(name);
    let account = root.join("_classic_era_/WTF/Account/ACCT1");
    std::fs::create_dir_all(account.join("SavedVariables")).expect("sv dir");
    std::fs::create_dir_all(account.join("Example Realm/Testchar")).expect("char dir");
    std::fs::write(account.join("SavedVariables/!BugGrabber.lua"), lua).expect("write sv");
    root
}

fn collector(root: &Path) -> AddonErrorsCollector {
    AddonErrorsCollector::new(root).with_utc_offset(0)
}

fn many_errors(n: usize) -> String {
    let mut lua = String::from("BugGrabberDB = {\n[\"session\"] = 7,\n[\"errors\"] = {\n");
    for i in 0..n {
        lua.push_str(&format!(
            "{{ [\"message\"] = \"Interface\\\\AddOns\\\\Foo\\\\F.lua:{i}: boom {i}\", \
             [\"time\"] = \"2026/11/18 21:30:15\", [\"session\"] = 7, [\"counter\"] = 1 }},\n"
        ));
    }
    lua.push_str("},\n}\n");
    lua
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn registry(cfg: &AppConfig) -> CollectorRegistry {
    CollectorRegistry::build(BUILTINS, cfg, Some(Platform::Windows), &no_env).expect("builtins")
}

#[test]
fn off_by_default_the_collector_lists_and_reads_nothing() {
    let root = wow_root("ae-off", FIXTURE);
    let mut cfg = AppConfig {
        wow_dir: root.display().to_string(),
        ..AppConfig::default()
    };
    let reg = registry(&cfg);
    let entry = reg.get(WOW_ADDON_ERRORS.id).expect("registered");
    assert!(!entry.enabled, "off until the user turns it on");
    assert_eq!(
        entry.roots,
        vec![root.clone()],
        "same WoW folder as LedgerLink"
    );
    assert!(
        entry.scan(ORIGIN, &SentState::default()).is_none(),
        "no scan, no read"
    );
    assert!(entry.watch_roots().is_empty(), "no listing while off");
    let sv = root.join("_classic_era_/WTF/Account/ACCT1/SavedVariables/!BugGrabber.lua");
    assert!(
        !entry.wants(&sv),
        "a change to the file is ignored while off"
    );

    cfg.set_collector_enabled(WOW_ADDON_ERRORS.id, true);
    let reg = registry(&cfg);
    let scan = reg
        .get(WOW_ADDON_ERRORS.id)
        .and_then(|e| e.scan(ORIGIN, &SentState::default()))
        .expect("on: scans");
    assert_eq!(scan.files, 1);
    assert_eq!(scan.pending.len(), 1, "the fixture fits one message");
}

#[test]
fn state_is_committed_only_after_the_server_settles_the_batch() {
    let root = wow_root("ae-ack", FIXTURE);
    let c = collector(&root);
    let rt = Runtime::new(super::temp_dir("ae-ack-state").join("sent.json"));
    let first = c.scan(ORIGIN, &rt.sent());
    let batch = first.pending.first().expect("one batch").clone();
    assert_eq!(batch.contract, CONTRACT);
    assert_eq!(batch.body["contract"], "wow-addon-errors");
    let text = batch.body.to_string();
    assert!(
        !text.contains("ExampleUser") && !text.contains("Testchar"),
        "{text}"
    );
    assert!(!text.contains("locals"), "locals never leave the machine");

    let failed = UploadOutcome::Deferred {
        reason: "server busy".into(),
        status: Some(503),
        code: None,
    };
    rt.finish(&batch, &effect(&failed));
    let again = c.scan(ORIGIN, &rt.sent());
    assert_eq!(
        again.pending.len(),
        1,
        "a failed upload is re-sent next scan"
    );
    assert_eq!(again.pending[0].body["payload"], batch.body["payload"]);
    assert_eq!(unsettled(&rt.sent(), &again.pending), 1);

    rt.finish(&batch, &effect(&UploadOutcome::Accepted));
    assert_eq!(unsettled(&rt.sent(), &first.pending), 0, "acked = settled");
    let after = c.scan(ORIGIN, &rt.sent());
    assert!(after.pending.is_empty(), "an unchanged file sends nothing");
    let other = c.scan("https://other.example", &rt.sent());
    assert_eq!(other.pending.len(), 1, "state is per server");
}

#[test]
fn a_grown_count_is_sent_again_and_only_that_record() {
    let root = wow_root("ae-grow", &many_errors(3));
    let c = collector(&root);
    let rt = Runtime::new(super::temp_dir("ae-grow-state").join("sent.json"));
    let batch = c.scan(ORIGIN, &rt.sent()).pending.remove(0);
    rt.finish(&batch, &effect(&UploadOutcome::Accepted));
    let grown = many_errors(3).replacen("[\"counter\"] = 1", "[\"counter\"] = 5", 1);
    let sv = root.join("_classic_era_/WTF/Account/ACCT1/SavedVariables/!BugGrabber.lua");
    std::fs::write(sv, grown).expect("rewrite");
    let next = c.scan(ORIGIN, &rt.sent());
    assert_eq!(next.pending.len(), 1);
    let records = next.pending[0].body["payload"]["records"]
        .as_array()
        .expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["count"], 5);
}

#[test]
fn many_errors_split_into_messages_of_at_most_fifty_records() {
    let root = wow_root("ae-split", &many_errors(120));
    let scan = collector(&root).scan(ORIGIN, &SentState::default());
    let sizes: Vec<usize> = scan
        .pending
        .iter()
        .map(|p| p.body["payload"]["records"].as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(sizes, vec![50, 50, 20]);
    let mut items: Vec<_> = scan.pending.iter().map(|p| p.item.clone()).collect();
    items.dedup();
    assert_eq!(items.len(), 3, "each batch has its own item");
    let recorded: usize = scan
        .pending
        .iter()
        .map(|p| record_items(&p.body).len())
        .sum();
    assert_eq!(recorded, 120, "settling records every record key");
    for p in &scan.pending {
        assert!(p.body.to_string().len() <= rl_collectors::ADDON_ERRORS_MAX_BYTES);
        assert_eq!(p.body["payload"]["wowFlavor"], "_classic_era_");
        assert_eq!(
            p.body["payload"]["sourceVersion"], "unknown",
            "no .toc here"
        );
    }
}

#[test]
fn the_registry_and_dev_bridge_report_the_collector() {
    let ids: Vec<_> = BUILTINS.iter().map(|b| b.manifest.id).collect();
    assert!(ids.contains(&"wow-addon-errors"), "{ids:?}");
    assert_eq!(WOW_ADDON_ERRORS.contract_id, CONTRACT);
    let manifest_default = WOW_ADDON_ERRORS.default_enabled;
    assert!(!manifest_default, "off by default");
    let snap = registry(&AppConfig::default()).snapshot();
    let row = snap
        .iter()
        .find(|s| s.id == "wow-addon-errors")
        .expect("snapshot row");
    assert_eq!(row.file_patterns, vec!["!BugGrabber.lua".to_owned()]);
    assert!(!row.enabled);

    let rt = Runtime::new(super::temp_dir("ae-bridge").join("sent.json"));
    let body = collectors_body(&rt.snapshot(&AppConfig::default()), false);
    assert_eq!(
        schema_errors("collectors", &body),
        Vec::<String>::new(),
        "{body}"
    );
    let rows = body["collectors"].as_array().expect("rows");
    let row = rows
        .iter()
        .find(|r| r["id"] == "wow-addon-errors")
        .expect("bridge row");
    assert_eq!(row["status"], "off");
}

#[test]
fn the_addon_errors_root_follows_the_wow_folder_unless_overridden() {
    let mut cfg = AppConfig {
        wow_dir: "D:\\Games\\WoW".into(),
        ..AppConfig::default()
    };
    assert_eq!(
        cfg.roots_override(WOW_ADDON_ERRORS.id),
        Some(vec!["D:\\Games\\WoW".to_owned()])
    );
    cfg.collector_roots
        .insert(WOW_ADDON_ERRORS.id.to_owned(), vec!["E:\\WoW".into()]);
    assert_eq!(
        cfg.roots_override(WOW_ADDON_ERRORS.id),
        Some(vec!["E:\\WoW".to_owned()])
    );
}

#[test]
fn the_account_folder_name_never_leaves_the_machine() {
    let lua = "BugGrabberDB = {\n[\"errors\"] = {\n{ [\"message\"] = \"WTF\\\\Account\\\\ACCT1\\\\SavedVariables\\\\Foo.lua:1: bad ACCT1\", [\"counter\"] = 1 },\n},\n}\n";
    let root = wow_root("ae-account", lua);
    let scan = collector(&root).scan(ORIGIN, &SentState::default());
    let body = scan.pending.first().expect("one batch").body.to_string();
    assert!(!body.contains("ACCT1"), "{body}");
    assert!(body.contains("<account>"), "{body}");
}

#[test]
fn a_scan_prunes_settled_errors_buggrabber_no_longer_holds() {
    let root = wow_root("ae-prune", &many_errors(3));
    let c = collector(&root);
    let first = c.scan(ORIGIN, &SentState::default());
    let scope = first.pending.first().expect("batch").scope.clone();
    let mut sent = SentState::default();
    for p in &first.pending {
        for (k, v) in record_items(&p.body) {
            sent.record(&scope, &k, &v);
        }
    }
    sent.record(&scope, "purged-key", "4");
    let again = c.scan(ORIGIN, &sent);
    assert!(again.pending.is_empty(), "all live errors already settled");
    let (s, keep) = again.retain.first().expect("prunes the purged key");
    assert_eq!(s, &scope);
    assert_eq!(keep.len(), 3);
    assert!(!keep.contains("purged-key"));
    assert!(sent.retain(&scope, keep));
    assert!(!sent.scope(&scope).contains_key("purged-key"));
    assert_eq!(sent.scope(&scope).len(), 3);
    assert!(
        c.scan(ORIGIN, &sent).retain.is_empty(),
        "nothing left to prune"
    );
}

#[test]
fn a_refused_batch_is_settled_and_logged_by_count_only() {
    let root = wow_root("ae-refused", &many_errors(2));
    let mut scan = collector(&root).scan(ORIGIN, &SentState::default());
    let p = scan.pending.remove(0);
    let fx = effect(&UploadOutcome::Rejected {
        status: 400,
        code: "invalid_payload".into(),
        issue: None,
    });
    assert!(
        fx.record,
        "settled: never re-sent, never blocks later uploads"
    );
    let note = refused_note(&p, &fx).expect("note");
    assert_eq!(
        note,
        "2 addon error(s) refused by the server and not re-sent"
    );
    assert!(!note.contains("boom"));
    assert!(refused_note(&p, &effect(&UploadOutcome::Accepted)).is_none());
}
