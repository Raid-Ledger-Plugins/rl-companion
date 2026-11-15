//! `LedgerLinkDB.dev.probes` parsing (ROK-2191). Inline synthetic
//! SavedVariables only: fake probe ids, no names.

use crate::{
    DEV_PROBE_RESULT_MAX_BYTES, SkipReason, WowSvError, is_probe_id, parse_ledgerlink_dev_probes,
};

const P1: &str = "01JBX7Q2M3N4P5R6S7T8V9W0XY";
const P2: &str = "01JBX7Q2M3N4P5R6S7T8V9W0XZ";

fn sv(dev: &str) -> Vec<u8> {
    format!("LedgerLinkDB = {{\n[\"outbox\"] = {{\n}},\n{dev}\n}}\n").into_bytes()
}

fn probes(entries: &str) -> Vec<u8> {
    sv(&format!(
        "[\"dev\"] = {{\n[\"snapshots\"] = {{\n}},\n[\"probes\"] = {{\n{entries}}},\n}},"
    ))
}

rl_test! {
    fn no_dev_or_probes_table_is_empty() {
        for bytes in [sv(""), b"OtherDB = {\n}\n".to_vec(), sv("[\"dev\"] = {\n},")] {
            let out = parse_ledgerlink_dev_probes(&bytes).expect("parses");
            assert!(out.results.is_empty() && out.skipped.is_empty(), "{out:?}");
        }
    }
}

rl_test! {
    fn non_table_probes_is_an_error() {
        let bytes = sv("[\"dev\"] = {\n[\"probes\"] = \"oops\",\n},");
        let err = parse_ledgerlink_dev_probes(&bytes).expect_err("non-table probes");
        assert!(
            matches!(err, WowSvError::NotATable { what: "LedgerLinkDB.dev.probes", .. }),
            "{err:?}"
        );
    }
}

rl_test! {
    fn reads_results_keyed_by_probe_id() {
        let bytes = probes(&format!(
            "[\"{P2}\"] = \"{{\\\"b\\\":2}}\",\n[\"{P1}\"] = \"{{\\\"a\\\":1}}\",\n"
        ));
        let out = parse_ledgerlink_dev_probes(&bytes).expect("parses");
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let got: Vec<(&str, &str)> =
            out.results.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(got, [(P1, "{\"a\":1}"), (P2, "{\"b\":2}")]);
    }
}

rl_test! {
    fn last_duplicate_key_wins() {
        let bytes = probes(&format!("[\"{P1}\"] = \"first\",\n[\"{P1}\"] = \"second\",\n"));
        let out = parse_ledgerlink_dev_probes(&bytes).expect("parses");
        assert_eq!(out.results.len(), 1);
        assert_eq!(out.results.get(P1).map(String::as_str), Some("second"));
    }
}

rl_test! {
    fn bad_key_non_string_and_oversize_are_skipped() {
        let big = "x".repeat(DEV_PROBE_RESULT_MAX_BYTES + 1);
        let bytes = probes(&format!(
            "[\"01jbx7q2m3n4p5r6s7t8v9w0xy\"] = \"lower\",\n[\"{P1}\"] = {{\n}},\n\"positional\", -- [1]\n[\"{P2}\"] = \"{big}\",\n"
        ));
        let out = parse_ledgerlink_dev_probes(&bytes).expect("parses");
        assert!(out.results.is_empty(), "{:?}", out.results.keys());
        let reasons: Vec<&SkipReason> = out.skipped.iter().map(|s| &s.reason).collect();
        assert_eq!(
            reasons,
            [
                &SkipReason::BadProbeId,
                &SkipReason::NotAString,
                &SkipReason::NonStringKey,
                &SkipReason::ProbeResultTooLarge { size: DEV_PROBE_RESULT_MAX_BYTES + 1 },
            ]
        );
    }
}

rl_test! {
    fn probe_id_is_a_crockford_ulid() {
        assert!(is_probe_id(P1));
        assert!(is_probe_id("0123456789ABCDEFGHJKMNPQRS"));
        for bad in ["", "01JBX7Q2M3N4P5R6S7T8V9W0X", "01JBX7Q2M3N4P5R6S7T8V9W0XYZ", "01JBX7Q2M3N4P5R6S7T8V9W0XI", "01JBX7Q2M3N4P5R6S7T8V9W0XU"] {
            assert!(!is_probe_id(bad), "{bad}");
        }
    }
}
