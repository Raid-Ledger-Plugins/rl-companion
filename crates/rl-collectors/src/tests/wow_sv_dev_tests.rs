//! `LedgerLinkDB.dev.snapshots` parsing (ROK-2072 §6). Inline synthetic
//! SavedVariables only: fake GUIDs, no names.

use crate::{
    DEV_SNAPSHOT_MAX_BYTES, SkipReason, WowSvError, is_player_guid, parse_ledgerlink_dev_snapshots,
};

const G1: &str = "Player-0000-0A000001";
const G2: &str = "Player-0000-0A000002";

fn sv(dev: &str) -> Vec<u8> {
    format!("LedgerLinkDB = {{\n[\"outbox\"] = {{\n}},\n{dev}\n}}\n").into_bytes()
}

rl_test! {
    fn no_dev_section_is_empty_not_an_error() {
        let out = parse_ledgerlink_dev_snapshots(&sv("")).expect("parses");
        assert!(out.entries.is_empty() && out.skipped.is_empty(), "{out:?}");
        let none = parse_ledgerlink_dev_snapshots(b"OtherDB = {\n}\n").expect("parses");
        assert!(none.entries.is_empty());
        let no_snaps = parse_ledgerlink_dev_snapshots(&sv("[\"dev\"] = {\n},")).expect("parses");
        assert!(no_snaps.entries.is_empty());
    }
}

rl_test! {
    fn reads_snapshot_strings_sorted_by_guid() {
        let body = format!(
            "[\"dev\"] = {{\n[\"snapshots\"] = {{\n[\"{G2}\"] = \"{{\\\"b\\\":2}}\",\n[\"{G1}\"] = \"{{\\\"a\\\":1}}\",\n}},\n}},"
        );
        let out = parse_ledgerlink_dev_snapshots(&sv(&body)).expect("parses");
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let got: Vec<(&str, &str)> =
            out.entries.iter().map(|e| (e.guid.as_str(), e.json.as_str())).collect();
        assert_eq!(got, [(G1, "{\"a\":1}"), (G2, "{\"b\":2}")]);
    }
}

rl_test! {
    fn last_duplicate_key_wins() {
        let body = format!(
            "[\"dev\"] = {{\n[\"snapshots\"] = {{\n[\"{G1}\"] = \"first\",\n[\"{G1}\"] = \"second\",\n}},\n}},"
        );
        let out = parse_ledgerlink_dev_snapshots(&sv(&body)).expect("parses");
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].json, "second");
    }
}

rl_test! {
    fn bad_guid_non_string_and_oversize_are_skipped() {
        let big = "x".repeat(DEV_SNAPSHOT_MAX_BYTES + 1);
        let body = format!(
            "[\"dev\"] = {{\n[\"snapshots\"] = {{\n[\"Player-0000-0a000001\"] = \"lower hex\",\n[\"{G1}\"] = {{\n}},\n\"positional\", -- [1]\n[\"{G2}\"] = \"{big}\",\n}},\n}},"
        );
        let out = parse_ledgerlink_dev_snapshots(&sv(&body)).expect("parses");
        assert!(out.entries.is_empty(), "{:?}", out.entries);
        let reasons: Vec<&SkipReason> = out.skipped.iter().map(|s| &s.reason).collect();
        assert_eq!(
            reasons,
            [
                &SkipReason::BadGuid,
                &SkipReason::NotAString,
                &SkipReason::NonStringKey,
                &SkipReason::SnapshotTooLarge { size: DEV_SNAPSHOT_MAX_BYTES + 1 },
            ]
        );
    }
}

rl_test! {
    fn exactly_the_cap_is_accepted() {
        let max = "y".repeat(DEV_SNAPSHOT_MAX_BYTES);
        let body = format!("[\"dev\"] = {{\n[\"snapshots\"] = {{\n[\"{G1}\"] = \"{max}\",\n}},\n}},");
        let out = parse_ledgerlink_dev_snapshots(&sv(&body)).expect("parses");
        assert_eq!(out.entries.len(), 1);
    }
}

rl_test! {
    fn non_table_dev_is_a_typed_error() {
        let err = parse_ledgerlink_dev_snapshots(&sv("[\"dev\"] = 5,")).expect_err("dev is a number");
        assert!(matches!(err, WowSvError::NotATable { what: "LedgerLinkDB.dev", .. }), "{err:?}");
        let err = parse_ledgerlink_dev_snapshots(&sv("[\"dev\"] = {\n[\"snapshots\"] = \"x\",\n},"))
            .expect_err("snapshots is a string");
        assert!(matches!(err, WowSvError::NotATable { what: "LedgerLinkDB.dev.snapshots", .. }));
    }
}

rl_test! {
    fn guid_shape() {
        assert!(is_player_guid(G1));
        assert!(is_player_guid("Player-12345-ABCDEF09"));
        for bad in ["Player--0A000001", "Player-123456-0A000001", "Player-1-0A00001", "Creature-1-0A000001", "Player-1-0a000001"] {
            assert!(!is_player_guid(bad), "{bad}");
        }
    }
}
