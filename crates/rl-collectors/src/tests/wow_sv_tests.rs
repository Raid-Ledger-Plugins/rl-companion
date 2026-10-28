//! `LedgerLink.lua` outbox parsing. Fixtures are synthetic SavedVariables
//! files built from the anonymised LedgerLink v1 contract fixtures (fake
//! names and GUIDs), compiled in with `include_bytes!`.

use std::collections::BTreeMap;

use crate::{
    SkipReason, WowSvError, changed_entries, is_valid_page_token, parse_ledgerlink_outbox,
    sha256_hex, validate_export_text,
};

pub(crate) const MULTI: &[u8] = include_bytes!("../../tests/fixtures/wow_sv/multi-entry.lua");
pub(crate) const LONG: &[u8] = include_bytes!("../../tests/fixtures/wow_sv/long-string.lua");
const EMPTY: &[u8] = include_bytes!("../../tests/fixtures/wow_sv/empty-outbox.lua");
const MISSING: &[u8] = include_bytes!("../../tests/fixtures/wow_sv/missing-outbox.lua");
const MALFORMED: &[u8] = include_bytes!("../../tests/fixtures/wow_sv/malformed.lua");
const BAD_ENTRIES: &[u8] = include_bytes!("../../tests/fixtures/wow_sv/bad-entries.lua");

const G1: &str = "Player-0000-0A000001";
const G2: &str = "Player-0000-0A000002";
const G3: &str = "Player-0000-0A000003";
const G4: &str = "Player-0000-0A000004";

rl_test! {
    fn reads_every_entry_of_a_multi_entry_outbox() {
        let outbox = parse_ledgerlink_outbox(MULTI).expect("multi-entry parses");
        assert!(outbox.skipped.is_empty(), "skipped: {:?}", outbox.skipped);
        let guids: Vec<&str> = outbox.entries.iter().map(|e| e.guid.as_str()).collect();
        assert_eq!(guids, [G1, G2, G3]);
        let [char_e, guild_e, all_e] = outbox.entries.as_slice() else {
            panic!("expected three entries, got {}", outbox.entries.len());
        };
        assert_eq!(char_e.exported_at, Some(1_790_000_000));
        assert_eq!(char_e.section.as_deref(), Some("char"));
        assert!(char_e.text.starts_with("!RL1!char!eJxt"), "{}", &char_e.text[..20]);
        assert_eq!(char_e.text.split_ascii_whitespace().count(), 1);
        assert_eq!(guild_e.section.as_deref(), Some("guild"));
        assert_eq!(guild_e.text.lines().count(), 3, "\\n escapes become newlines");
        assert!(guild_e.text.lines().any(|l| l.starts_with("!RL1!guild-3of3!")));
        assert_eq!(all_e.exported_at, Some(1_790_000_200));
        assert_eq!(all_e.text.lines().count(), 5, "backslash-newline becomes a newline");
        for e in &outbox.entries {
            assert_eq!(e.sha256, sha256_hex(e.text.as_bytes()));
            assert!(!e.text.contains('\\'), "an escape survived in {}", e.guid);
        }
    }
}

rl_test! {
    fn reads_long_bracket_strings_and_drops_the_leading_newline() {
        let long = parse_ledgerlink_outbox(LONG).expect("long-string parses");
        let multi = parse_ledgerlink_outbox(MULTI).expect("multi-entry parses");
        let guids: Vec<&str> = long.entries.iter().map(|e| e.guid.as_str()).collect();
        assert_eq!(guids, [G1, G4]);
        let (Some(long_char), Some(multi_char)) = (long.entries.first(), multi.entries.first())
        else {
            panic!("missing char entry");
        };
        assert_eq!(long_char.text, multi_char.text, "[==[ and quoted forms differ");
        assert_eq!(long_char.sha256, multi_char.sha256);
        assert!(long.entries.get(1).is_some_and(|e| e.text.starts_with("!RL1!raid!")));
    }
}

rl_test! {
    fn empty_or_missing_outbox_yields_nothing() {
        for (name, bytes) in [("empty", EMPTY), ("missing", MISSING)] {
            let outbox = parse_ledgerlink_outbox(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(outbox.entries.is_empty() && outbox.skipped.is_empty(), "{name}: {outbox:?}");
        }
        let none = parse_ledgerlink_outbox(b"LedgerLinkCharDB = {\n[\"ruleset\"] = \"normal\",\n}\n");
        assert_eq!(none.map(|o| o.entries.len()), Ok(0));
        assert_eq!(parse_ledgerlink_outbox(b"").map(|o| o.entries.len()), Ok(0));
        assert_eq!(parse_ledgerlink_outbox(b"\xEF\xBB\xBF\n").map(|o| o.entries.len()), Ok(0));
    }
}

rl_test! {
    fn refuses_a_function_call_with_a_typed_error() {
        match parse_ledgerlink_outbox(MALFORMED) {
            Err(WowSvError::Unexpected { what: "a value", found, .. }) => {
                assert_eq!(found, "'o'");
            }
            other => panic!("expected Unexpected, got {other:?}"),
        }
    }
}

rl_test! {
    fn refuses_unsupported_syntax() {
        let cases: [(&[u8], &str); 8] = [
            (b"LedgerLinkDB = {", "truncated table"),
            (b"LedgerLinkDB = { [\"a\"] = \"x }", "unterminated string"),
            (b"LedgerLinkDB = { [\"a\"] = \"x\ny\" }", "raw newline in string"),
            (b"LedgerLinkDB = { [\"a\"] = \"\\q\" }", "bad escape"),
            (b"LedgerLinkDB = { [\"a\"] = 0x10 }", "hex number"),
            (b"LedgerLinkDB = { [nil] = 1 }", "nil key"),
            (b"LedgerLinkDB = 1 + 2", "operator"),
            (b"LedgerLinkDB = { [\"a\"] = [==[ x ]=] }", "unclosed long string"),
        ];
        for (src, what) in cases {
            assert!(parse_ledgerlink_outbox(src).is_err(), "{what} parsed");
        }
    }
}

rl_test! {
    fn deep_nesting_is_an_error_not_a_stack_overflow() {
        let src = format!("LedgerLinkDB = {}", "{".repeat(100_000));
        match parse_ledgerlink_outbox(src.as_bytes()) {
            Err(WowSvError::TooDeep { limit: 64, .. }) => {}
            other => panic!("expected TooDeep, got {other:?}"),
        }
    }
}

rl_test! {
    fn non_table_db_or_outbox_is_an_error() {
        let db = parse_ledgerlink_outbox(b"LedgerLinkDB = \"x\"");
        assert_eq!(db, Err(WowSvError::NotATable { what: "LedgerLinkDB", found: "string" }));
        let ob = parse_ledgerlink_outbox(b"LedgerLinkDB = { outbox = 5 }");
        assert_eq!(ob, Err(WowSvError::NotATable { what: "LedgerLinkDB.outbox", found: "number" }));
    }
}

rl_test! {
    fn drops_bad_entries_and_keeps_good_ones() {
        let outbox = parse_ledgerlink_outbox(BAD_ENTRIES).expect("bad-entries parses");
        let guids: Vec<&str> = outbox.entries.iter().map(|e| e.guid.as_str()).collect();
        assert_eq!(guids, [G1]);
        let reasons: Vec<(&str, &SkipReason)> =
            outbox.skipped.iter().map(|s| (s.key.as_str(), &s.reason)).collect();
        assert_eq!(
            reasons,
            [
                ("Player-0000-0A000005", &SkipReason::InvalidToken { index: 1 }),
                ("Player-0000-0A000006", &SkipReason::MissingText),
                ("Player-0000-0A000007", &SkipReason::NotATable),
                ("[1]", &SkipReason::NonStringKey),
            ]
        );
    }
}

rl_test! {
    fn last_duplicate_key_wins_and_lua_syntax_variants_parse() {
        let src = br#"-- header comment
--[==[ long
comment ]==]
LedgerLinkDB = {
  outbox = {
    ["P-1"] = { text = '!RL1!raid!QUJD', exportedAt = 1.5, section = nil; },
    ["P-1"] = { text = "!RL1!raid!\x51\85JE\z
                 ", exportedAt = 2e3 },
  };
  region = -3, [true] = false, [2.5] = { 1, 2, [[x]] },
};"#;
        let outbox = parse_ledgerlink_outbox(src).expect("variants parse");
        let [entry] = outbox.entries.as_slice() else {
            panic!("expected one entry, got {:?}", outbox.entries);
        };
        assert_eq!(entry.text, "!RL1!raid!QUJE");
        assert_eq!(entry.exported_at, Some(2000));
        assert_eq!(entry.section, None);
    }
}

rl_test! {
    fn page_token_grammar_matches_the_contract_regex() {
        for ok in ["!RL1!char!QUJD", "!RL12!guild-2of8!QQ==", "!RL1!raid!a+/9=", "!RL1!guild-1of1!x"] {
            assert!(is_valid_page_token(ok), "{ok} rejected");
        }
        for bad in [
            "", "RL1!char!QUJD", "!RL!char!QUJD", "!RL1!chars!QUJD", "!RL1!char!", "!RL1!char!===",
            "!RL1!char!QQ===", "!RL1!guild-of3!QQ", "!RL1!guild-1of!QQ", "!RL1!guild-1x3!QQ",
            "!RL1!char!QU-D", "!RL1!all!QUJD", "!RL1!char!QUJD ",
        ] {
            assert!(!is_valid_page_token(bad), "{bad:?} accepted");
        }
        assert_eq!(validate_export_text("  \n "), Err(SkipReason::EmptyText));
        assert_eq!(validate_export_text("!RL1!raid!QUJD\n!RL1!char!QUJD"), Ok(()));
        let huge = format!("!RL1!raid!{}", "Q".repeat(262_144));
        assert!(matches!(validate_export_text(&huge), Err(SkipReason::TextTooLarge { .. })));
    }
}

rl_test! {
    fn changed_entries_returns_new_and_modified_guids_only() {
        let outbox = parse_ledgerlink_outbox(MULTI).expect("multi-entry parses");
        let all: Vec<&str> =
            changed_entries(&outbox.entries, &BTreeMap::new()).iter().map(|e| e.guid.as_str()).collect();
        assert_eq!(all, [G1, G2, G3]);
        let mut previous: BTreeMap<String, String> =
            outbox.entries.iter().map(|e| (e.guid.clone(), e.sha256.clone())).collect();
        assert!(changed_entries(&outbox.entries, &previous).is_empty());
        previous.insert(G2.to_string(), "stale".to_string());
        previous.remove(G3);
        previous.insert("Player-0000-0AFFFFFF".to_string(), "gone".to_string());
        let changed: Vec<&str> =
            changed_entries(&outbox.entries, &previous).iter().map(|e| e.guid.as_str()).collect();
        assert_eq!(changed, [G2, G3]);
    }
}

const ADDON_PR35: &[u8] = include_bytes!("../../tests/fixtures/wow_sv/addon-pr35-shape.lua");

rl_test! {
    fn reads_the_ledger_link_pr35_outbox_shape() {
        let outbox = parse_ledgerlink_outbox(ADDON_PR35).expect("PR #35 shape parses");
        assert!(outbox.skipped.is_empty(), "skipped: {:?}", outbox.skipped);
        let [logout, click] = outbox.entries.as_slice() else {
            panic!("expected two entries, got {:?}", outbox.entries);
        };
        assert_eq!(logout.guid, G1);
        assert_eq!(logout.source.as_deref(), Some("logout"));
        assert_eq!(logout.section.as_deref(), Some("char"));
        assert_eq!(logout.exported_at, Some(1_790_003_599));
        assert_eq!(logout.stored_at, Some(1_790_003_600));
        assert_eq!(click.guid, G3);
        assert_eq!(click.source.as_deref(), Some("export"));
        assert_eq!(click.section.as_deref(), Some("all"));
        assert_eq!(click.text.lines().count(), 5);
        let multi = parse_ledgerlink_outbox(MULTI).expect("multi-entry parses");
        let same_text: Vec<bool> = [G1, G3]
            .iter()
            .map(|g| {
                let a = outbox.entries.iter().find(|e| e.guid == *g).map(|e| &e.sha256);
                let b = multi.entries.iter().find(|e| e.guid == *g).map(|e| &e.sha256);
                a.is_some() && a == b
            })
            .collect();
        assert_eq!(same_text, [true, true], "sha256 is over the decoded text");
    }
}
