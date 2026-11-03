//! ROK-2072 PR-C: the Rust validator agrees with every pinned
//! `ledgerlink-snapshot*` fixture (valid accepted, invalid rejected at the
//! `invalid/EXPECTED.json` path) and the page/history bodies match `$defs`.

use super::dev_bridge_unit_tests::schema_errors;
use crate::dev_bridge::snapshot_validate::{validate_slice, validate_snapshot};
use serde_json::Value;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/dev-bridge/v1/fixtures")
}

fn load(rel: &str) -> Value {
    let text = std::fs::read_to_string(fixtures().join(rel)).expect(rel);
    serde_json::from_str(&text).expect(rel)
}

/// Fixture stems under `dir` starting with `prefix`.
fn stems(dir: &str, prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(fixtures().join(dir))
        .expect("fixtures dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".json").map(str::to_owned))
        .filter(|n| n.starts_with(prefix))
        .collect();
    out.sort();
    out
}

fn def_for(stem: &str) -> &'static str {
    if stem.starts_with("ledgerlink-snapshot-page") {
        "ledgerlink-snapshot-page"
    } else if stem.starts_with("ledgerlink-snapshot-history") {
        "ledgerlink-snapshot-history"
    } else {
        "ledgerlink-snapshot"
    }
}

#[test]
fn every_valid_snapshot_fixture_is_accepted() {
    let names = stems("", "ledgerlink-snapshot");
    assert!(names.len() >= 6, "{names:?}");
    for stem in names {
        let v = load(&format!("{stem}.json"));
        let def = def_for(&stem);
        assert_eq!(schema_errors(def, &v), Vec::<String>::new(), "{stem}");
        let res = match def {
            "ledgerlink-snapshot" => validate_snapshot(&v),
            "ledgerlink-snapshot-page" => validate_slice(&v["snapshot"]),
            _ => Ok(()),
        };
        assert_eq!(res, Ok(()), "{stem}");
        assert!(serde_json::to_vec(&v).unwrap().len() <= 49_152, "{stem}");
    }
}

#[test]
fn every_invalid_snapshot_fixture_is_rejected_at_its_expected_path() {
    let expected = load("invalid/EXPECTED.json");
    let names = stems("invalid", "ledgerlink-snapshot");
    assert!(names.len() >= 9, "{names:?}");
    for stem in names {
        let v = load(&format!("invalid/{stem}.json"));
        let want = expected[&stem]["path"].as_str().expect("EXPECTED entry");
        let def = def_for(&stem);
        if def != "ledgerlink-snapshot" {
            assert!(
                !schema_errors(def, &v).is_empty(),
                "{stem} passed $defs/{def}"
            );
            continue;
        }
        let at = if want.is_empty() { "root" } else { want };
        match validate_snapshot(&v) {
            Ok(()) => panic!("{stem}: accepted, expected an issue at {at}"),
            Err(e) => assert!(
                e.starts_with(&format!("{at}:")),
                "{stem}: want {at}, got {e}"
            ),
        }
    }
}
