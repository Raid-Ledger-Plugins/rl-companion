//! Larian stats `.txt` reader and `using` resolution (ROK-2081). Text is
//! hand-written in the U5 shape with invented ids.

use std::fmt::Write as _;

use crate::{
    CatalogueError, ResolvedItem, STATS_CHECK_EVERY_LINES, STATS_MAX_ENTRIES, STATS_MAX_ID_BYTES,
    StatsEntry, USING_MAX_DEPTH, parse_stats, parse_stats_with, resolve_using,
};

const CHAIN: &str = "// header comment\r\n\
new entry \"ARM_Base\"\r\n\
type \"Armor\"\r\n\
data \"RootTemplate\" \"00000000-0000-0000-0000-00000000base\"\r\n\
data \"Rarity\" \"Common\"\r\n\
\r\n\
new entry \"ARM_Mid\"\r\n\
type \"Armor\"\r\n\
using \"ARM_Base\"\r\n\
data \"Rarity\" \"Uncommon\"\r\n\
data \"Weight\" \"1\"\r\n\
\r\n\
new entry \"ARM_TestHat\"\r\n\
using \"ARM_Mid\"\r\n\
  data \"Rarity\" \"Rare\"  \r\n\
new entry \"ARM_PlainHat\"\r\n\
using \"ARM_Mid\"\r\n\
data \"Rarity\" \"\"\r\n";

fn item(id: &str, kind: &str, rt: Option<&str>, rarity: Option<&str>) -> ResolvedItem {
    ResolvedItem {
        id: id.into(),
        kind: kind.into(),
        root_template: rt.map(Into::into),
        rarity: rarity.map(Into::into),
    }
}

fn find<'a>(items: &'a [ResolvedItem], id: &str) -> &'a ResolvedItem {
    items
        .iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("{id} not resolved: {items:?}"))
}

fn parse_reason(text: &str) -> (usize, &'static str) {
    match parse_stats(text) {
        Err(CatalogueError::Parse { at, reason, .. }) => (at, reason),
        other => panic!("expected a Parse error, got {other:?}"),
    }
}

rl_test! {
    fn stats_using_chain_three_deep_with_override_crlf_and_comments() {
        let entries = parse_stats(CHAIN).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[1].using.as_deref(), Some("ARM_Base"));
        let items = resolve_using(&entries);
        let base_rt = Some("00000000-0000-0000-0000-00000000base");
        assert_eq!(find(&items, "ARM_TestHat"), &item("ARM_TestHat", "Armor", base_rt, Some("Rare")));
        // Empty Rarity counts as unset: inherited from the nearest ancestor.
        assert_eq!(find(&items, "ARM_PlainHat").rarity.as_deref(), Some("Uncommon"));
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["ARM_Base", "ARM_Mid", "ARM_PlainHat", "ARM_TestHat"], "sorted by id");
    }
}

rl_test! {
    fn stats_cycle_and_missing_parent_keep_own_fields() {
        let text = "new entry \"A\"\ntype \"Weapon\"\nusing \"B\"\ndata \"Rarity\" \"Rare\"\n\
                    new entry \"B\"\ntype \"Weapon\"\nusing \"A\"\n\
                    new entry \"C\"\ntype \"Object\"\nusing \"Nowhere\"\ndata \"Rarity\" \"Epic\"\n\
                    new entry \"Self\"\ntype \"Object\"\nusing \"Self\"\n";
        let items = resolve_using(&parse_stats(text).unwrap());
        assert_eq!(find(&items, "A"), &item("A", "Weapon", None, Some("Rare")));
        assert_eq!(find(&items, "B"), &item("B", "Weapon", None, Some("Rare")), "B inherits A, then stops");
        assert_eq!(find(&items, "C"), &item("C", "Object", None, Some("Epic")));
        assert_eq!(find(&items, "Self"), &item("Self", "Object", None, None));
    }
}

rl_test! {
    fn stats_only_item_types_and_other_blocks_close_the_entry() {
        let text = "new entry \"Char\"\ntype \"Character\"\ndata \"Rarity\" \"Rare\"\n\
                    new entry \"NoType\"\ndata \"Rarity\" \"Rare\"\n\
                    new entry \"OBJ_Hat\"\ntype \"Object\"\n\
                    new equipment \"EQ_Set\"\ndata \"Rarity\" \"Legendary\"\nadd initialweaponset \"Melee\"\n\
                    new entry \"WPN_Inherits\"\nusing \"OBJ_Hat\"\n";
        let entries = parse_stats(text).unwrap();
        assert_eq!(entries.len(), 4, "the equipment block is not an entry: {entries:?}");
        assert_eq!(entries[2].rarity, None, "a line after `new equipment` must not attach to OBJ_Hat");
        let items = resolve_using(&entries);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["OBJ_Hat", "WPN_Inherits"], "type is inherited too");
    }
}

rl_test! {
    fn stats_later_duplicate_id_wins() {
        let text = "new entry \"X\"\ntype \"Armor\"\ndata \"Rarity\" \"Common\"\n\
                    new entry \"X\"\ntype \"Armor\"\ndata \"Rarity\" \"Legendary\"\n";
        let items = resolve_using(&parse_stats(text).unwrap());
        assert_eq!(items, [item("X", "Armor", None, Some("Legendary"))]);
    }
}

rl_test! {
    fn stats_malformed_lines_are_errors_with_line_numbers() {
        assert_eq!(parse_reason("new entry \"A\ntype \"Armor\""), (1, "unterminated quoted string"));
        assert_eq!(parse_reason("new entry \"A\"\ndata \"Rarity\""), (2, "expected a quoted string"));
        assert_eq!(parse_reason("new entry \"A\"\ntype Armor"), (2, "expected a quoted string"));
        assert_eq!(
            parse_reason("new entry \"A\"\n\nusing \"B\" \"C\""),
            (3, "unexpected text after the quoted strings")
        );
        // Outside an entry, keyword lines are ignored, not errors.
        assert_eq!(parse_stats("data \"broken\ntype oops\n").unwrap(), Vec::<StatsEntry>::new());
    }
}

rl_test! {
    fn stats_id_length_cap_at_edge_and_edge_plus_one() {
        let at = "A".repeat(STATS_MAX_ID_BYTES);
        let over = "B".repeat(STATS_MAX_ID_BYTES + 1);
        let text = format!("new entry \"{at}\"\ntype \"Armor\"\nnew entry \"{over}\"\ntype \"Armor\"\n");
        let entries = parse_stats(&text).unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, [at.as_str()], "the over-long id must be skipped");
    }
}

rl_test! {
    fn stats_entry_count_cap_at_edge_and_edge_plus_one() {
        let mut text = String::new();
        for i in 0..STATS_MAX_ENTRIES {
            writeln!(text, "new entry \"E{i}\"").unwrap();
        }
        assert_eq!(parse_stats(&text).unwrap().len(), STATS_MAX_ENTRIES);
        text.push_str("new entry \"OneMore\"\n");
        assert_eq!(
            parse_stats(&text),
            Err(CatalogueError::TooLarge {
                what: "stats entry count",
                size: STATS_MAX_ENTRIES as u64 + 1,
                limit: STATS_MAX_ENTRIES as u64,
            })
        );
    }
}

/// `E0` (an Armor) whose only rarity sits `hops` `using` links up.
fn chain(hops: usize) -> String {
    let mut text = String::from("new entry \"E0\"\ntype \"Armor\"\nusing \"E1\"\n");
    for i in 1..hops {
        writeln!(text, "new entry \"E{i}\"\nusing \"E{}\"", i + 1).unwrap();
    }
    writeln!(text, "new entry \"E{hops}\"\ndata \"Rarity\" \"Epic\"").unwrap();
    text
}

rl_test! {
    fn stats_using_depth_cap_at_edge_and_edge_plus_one() {
        let at = resolve_using(&parse_stats(&chain(USING_MAX_DEPTH)).unwrap());
        assert_eq!(find(&at, "E0").rarity.as_deref(), Some("Epic"), "{USING_MAX_DEPTH} hops must resolve");
        let over = resolve_using(&parse_stats(&chain(USING_MAX_DEPTH + 1)).unwrap());
        assert_eq!(find(&over, "E0").rarity, None, "{} hops must stop short", USING_MAX_DEPTH + 1);
    }
}

rl_test! {
    fn stats_stop_check_cadence_at_edge_and_edge_plus_one() {
        let lines = |n: usize| "// filler\n".repeat(n);
        let mut calls = 0;
        parse_stats_with(&lines(STATS_CHECK_EVERY_LINES), &mut || { calls += 1; false }).unwrap();
        assert_eq!(calls, 0, "no check inside the first {STATS_CHECK_EVERY_LINES} lines");
        let mut calls = 0;
        parse_stats_with(&lines(STATS_CHECK_EVERY_LINES + 1), &mut || { calls += 1; false }).unwrap();
        assert_eq!(calls, 1, "one check once line {} is reached", STATS_CHECK_EVERY_LINES + 1);
        let r = parse_stats_with(&lines(STATS_CHECK_EVERY_LINES + 1), &mut || true);
        assert_eq!(r, Err(CatalogueError::Timeout));
    }
}
