//! Catalogue join: stats → template → loca, LSX ids → loca, stats-only
//! rarity (ROK-2081 L3). Stats, LSX and loca are synthetic, invented ids.

use std::collections::{HashMap, HashSet};

use super::bg3_loca_tests::{h, loca};
use crate::{
    CAT_MAX_MATCHED_ENTRIES, CAT_MAX_ROWS, CatalogueBuilder, CatalogueCounts, CatalogueError,
    EntryKind, ItemName, STATS_CHECK_EVERY_LINES, parse_loca_wanted,
};

const TPL_HAT: &str = "0a1b2c3d-0000-4e5f-8a9b-00000000a001";
const TPL_SWORD: &str = "0a1b2c3d-0000-4e5f-8a9b-00000000a002";

fn armor_txt() -> String {
    format!(
        "new entry \"ARM_Base\"\ntype \"Armor\"\ndata \"Rarity\" \"Uncommon\"\n\n\
         new entry \"ARM_TestHat\"\nusing \"ARM_Base\"\ndata \"RootTemplate\" \"{TPL_HAT}\"\n\n\
         new entry \"ARM_Plain\"\ntype \"Armor\"\n\n\
         new entry \"ARM_Odd\"\ntype \"Armor\"\ndata \"Rarity\" \"Mythic\"\n"
    )
}

/// A second file whose entry inherits from `Armor.txt` (cross-file `using`).
fn weapon_txt() -> String {
    format!(
        "new entry \"WPN_TestSword\"\ntype \"Weapon\"\ndata \"RootTemplate\" \"{TPL_SWORD}\"\n\
         data \"Rarity\" \"rare\"\n\n\
         new entry \"WPN_Borrowed\"\nusing \"ARM_Base\"\n"
    )
}

fn lsx(node: &str, key: &str, handle_attr: &str, rows: &[(&str, &str)]) -> String {
    let mut s = String::from("<save><region id=\"x\"><node id=\"root\"><children>\n");
    for (k, hd) in rows {
        s += &format!(
            "<node id=\"{node}\"><attribute id=\"{key}\" type=\"FixedString\" value=\"{k}\" />\
             <attribute id=\"{handle_attr}\" type=\"TranslatedString\" handle=\"{hd}\" version=\"1\" /></node>\n"
        );
    }
    s + "</children></node></region></save>\n"
}

fn never() -> impl FnMut() -> bool {
    || false
}

fn row(name: Option<&str>, rarity: Option<&str>) -> ItemName {
    ItemName {
        name: name.map(Into::into),
        rarity: rarity.map(Into::into),
    }
}

fn too_large(r: Result<(), CatalogueError>) -> (&'static str, u64, u64) {
    match r {
        Err(CatalogueError::TooLarge { what, size, limit }) => (what, size, limit),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

/// Stats + templates + origins + quests fed in, the wanted set read from
/// a synthetic loca, then finished.
fn full_build() -> (HashSet<String>, crate::CatalogueTables, CatalogueCounts) {
    let mut b = CatalogueBuilder::new();
    b.add_stats(&armor_txt(), &mut never()).unwrap();
    b.add_stats(&weapon_txt(), &mut never()).unwrap();
    let tpl = [(TPL_HAT.to_uppercase(), h(1)), (TPL_SWORD.into(), h(2))];
    b.add_template_handles(tpl).unwrap();
    let ori = lsx(
        "Origin",
        "Name",
        "DisplayName",
        &[("TestOrigin", &h(3)), ("Ghost", &h(9))],
    );
    b.add_lsx(EntryKind::Origins, &ori).unwrap();
    let q = lsx(
        "Quest",
        "QuestID",
        "QuestTitle",
        &[("TST_Quest", &h(4)), ("TST_Leak", &h(5))],
    );
    b.add_lsx(EntryKind::Quests, &q).unwrap();
    let join = b.into_join();
    let wanted = join.wanted_handles().clone();
    let (k1, k2, k3, k4, k5) = (h(1), h(2), h(3), h(4), h(5));
    let bytes = loca(&[
        (k1.as_bytes(), b"<LSTag Tooltip=\"t\">Test</LSTag> Hat"),
        (k2.as_bytes(), b"Test Sword"),
        (k3.as_bytes(), b"Test Origin"),
        (k4.as_bytes(), b"A Test Quest"),
        (k5.as_bytes(), b"Leak 0a1b2c3d-0000-4e5f-8a9b-0c1d2e3f4a5b"),
    ]);
    let texts = parse_loca_wanted(&bytes, &wanted).unwrap();
    let (tables, counts) = join.finish(&texts);
    (wanted, tables, counts)
}

rl_test! {
    fn join_wants_exactly_the_handles_the_tables_need() {
        let (wanted, _, _) = full_build();
        let expect: HashSet<String> = [1, 2, 3, 4, 5, 9].into_iter().map(h).collect();
        assert_eq!(wanted, expect, "template handles of resolved items + every LSX handle");
    }
}

rl_test! {
    fn join_names_items_origins_and_quests_through_loca() {
        let (_, t, _) = full_build();
        assert_eq!(t.lookup_item("ARM_TestHat"), Some(&row(Some("Test Hat"), Some("Uncommon"))));
        assert_eq!(t.lookup_item("WPN_TestSword"), Some(&row(Some("Test Sword"), Some("Rare"))));
        assert_eq!(t.lookup_origin("TestOrigin"), Some("Test Origin"));
        assert_eq!(t.lookup_quest("TST_Quest"), Some("A Test Quest"));
        assert_eq!(t.lookup_origin("Ghost"), None, "handle missing from loca → no row");
        assert_eq!(t.lookup_quest("TST_Leak"), None, "GUID-bearing text → D9 refuses it");
        assert!(t.waypoints.is_empty(), "no waypoint source yet (ruling 11)");
    }
}

rl_test! {
    fn rarity_is_stats_only_and_rows_need_a_name_or_a_rarity() {
        let (_, t, counts) = full_build();
        assert_eq!(t.lookup_item("WPN_Borrowed"), Some(&row(None, Some("Uncommon"))), "cross-file using");
        assert_eq!(t.lookup_item("ARM_Plain"), None, "no name and no rarity → no row");
        assert_eq!(t.lookup_item("ARM_Odd"), None, "unknown rarity is None → no row");
        assert_eq!(t.lookup_item("ARM_Base"), Some(&row(None, Some("Uncommon"))));
        let want = CatalogueCounts { items: 4, items_named: 2, origins: 1, quests: 1, waypoints: 0, skipped: 2 };
        assert_eq!(counts, want);
    }
}

rl_test! {
    fn without_template_handles_names_stay_none_until_rok_2080() {
        let mut b = CatalogueBuilder::new();
        b.add_stats(&armor_txt(), &mut never()).unwrap();
        let join = b.into_join();
        assert!(join.wanted_handles().is_empty(), "no templates → no item handle wanted");
        let (t, counts) = join.finish(&HashMap::new());
        assert_eq!(t.lookup_item("ARM_TestHat"), Some(&row(None, Some("Uncommon"))));
        assert_eq!((counts.items, counts.items_named, counts.skipped), (2, 0, 0));
    }
}

rl_test! {
    fn later_lsx_duplicate_wins_and_non_lsx_kind_is_refused() {
        let mut b = CatalogueBuilder::new();
        b.add_lsx(EntryKind::Origins, &lsx("Origin", "Name", "DisplayName", &[("O", &h(1))])).unwrap();
        b.add_lsx(EntryKind::Origins, &lsx("Origin", "Name", "DisplayName", &[("O", &h(2))])).unwrap();
        assert!(matches!(b.add_lsx(EntryKind::StatsTxt, ""), Err(CatalogueError::Parse { .. })));
        let k2 = h(2);
        let texts = HashMap::from([(h(1), "First".to_owned()), (k2, "Second".to_owned())]);
        let (t, _) = b.into_join().finish(&texts);
        assert_eq!(t.lookup_origin("O"), Some("Second"));
    }
}

rl_test! {
    fn add_stats_honours_the_deadline_check() {
        let text = "// filler\n".repeat(STATS_CHECK_EVERY_LINES + 1);
        let mut b = CatalogueBuilder::new();
        assert_eq!(b.add_stats(&text, &mut || true), Err(CatalogueError::Timeout));
    }
}

rl_test! {
    fn matched_entry_cap_at_edge_and_edge_plus_one() {
        let mut b = CatalogueBuilder::new();
        for _ in 0..CAT_MAX_MATCHED_ENTRIES {
            b.add_template_handles([]).unwrap();
        }
        let (what, size, limit) = too_large(b.add_template_handles([]));
        assert_eq!((what, size, limit), ("matched entry count", 4_097, 4_096));
    }
}

rl_test! {
    fn row_cap_at_edge_and_edge_plus_one() {
        let pair = |i: usize| (format!("guid-{i}"), h(i as u32));
        let mut b = CatalogueBuilder::new();
        b.add_template_handles((0..CAT_MAX_ROWS).map(pair)).unwrap();
        let (what, size, limit) = too_large(b.add_template_handles([pair(CAT_MAX_ROWS)]));
        assert_eq!((what, size, limit), ("template handles", 200_001, 200_000));
    }
}

rl_test! {
    fn lsx_keys_that_are_not_safe_payload_keys_are_dropped_and_counted() {
        let bad_hex = "0a1b2c3d00004e5f8a9b0c1d2e3f4a5b";
        let rows = [("Good_1", h(1)), ("Bad-Key", h(2)), (bad_hex, h(3)), ("P1234567890", h(4))];
        let refs: Vec<(&str, &str)> = rows.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let mut b = CatalogueBuilder::new();
        b.add_lsx(EntryKind::Quests, &lsx("Quest", "QuestID", "QuestTitle", &refs)).unwrap();
        let join = b.into_join();
        assert_eq!(join.wanted_handles(), &HashSet::from([h(1)]), "refused keys want no handle");
        let texts: HashMap<String, String> = (1..=4).map(|i| (h(i), format!("Quest {i}"))).collect();
        let (t, counts) = join.finish(&texts);
        assert_eq!(t.quests.keys().collect::<Vec<_>>(), ["Good_1"]);
        assert_eq!((counts.quests, counts.skipped), (1, 3), "each refused key is skipped");
    }
}

rl_test! {
    fn stats_ids_holding_an_id_leak_are_dropped_and_counted() {
        let txt = format!(
            "new entry \"{}\"\ntype \"Armor\"\ndata \"Rarity\" \"Rare\"\n\n\
             new entry \"ARM_Ok\"\ntype \"Armor\"\ndata \"Rarity\" \"Rare\"\n",
            "a".repeat(32)
        );
        let mut b = CatalogueBuilder::new();
        b.add_stats(&txt, &mut never()).unwrap();
        let (t, counts) = b.into_join().finish(&HashMap::new());
        assert_eq!(t.items.keys().collect::<Vec<_>>(), ["ARM_Ok"]);
        assert_eq!((counts.items, counts.skipped), (1, 1));
    }
}

rl_test! {
    fn later_lsx_duplicate_wins_even_when_its_text_is_missing_or_refused() {
        let ori = |k: &str| lsx("Origin", "Name", "DisplayName", &[("O", k)]);
        for later_text in [None, Some("Leak 0a1b2c3d-0000-4e5f-8a9b-0c1d2e3f4a5b")] {
            let mut b = CatalogueBuilder::new();
            b.add_lsx(EntryKind::Origins, &ori(&h(1))).unwrap();
            b.add_lsx(EntryKind::Origins, &ori(&h(2))).unwrap();
            let join = b.into_join();
            assert_eq!(join.wanted_handles(), &HashSet::from([h(2)]), "the overridden handle is not wanted");
            let mut texts = HashMap::from([(h(1), "First".to_owned())]);
            if let Some(t) = later_text {
                texts.insert(h(2), t.to_owned());
            }
            let (t, counts) = join.finish(&texts);
            assert_eq!(t.lookup_origin("O"), None, "earlier copy must not survive ({later_text:?})");
            assert_eq!((counts.origins, counts.skipped), (0, 1));
        }
    }
}

rl_test! {
    fn later_stats_duplicate_wins_even_when_its_name_is_missing() {
        let first = format!("new entry \"ARM_Dup\"\ntype \"Armor\"\ndata \"RootTemplate\" \"{TPL_HAT}\"\n");
        let later = format!("new entry \"ARM_Dup\"\ntype \"Armor\"\ndata \"RootTemplate\" \"{TPL_SWORD}\"\n");
        let mut b = CatalogueBuilder::new();
        b.add_stats(&first, &mut never()).unwrap();
        b.add_stats(&later, &mut never()).unwrap();
        b.add_template_handles([(TPL_HAT.into(), h(1)), (TPL_SWORD.into(), h(2))]).unwrap();
        let join = b.into_join();
        assert_eq!(join.wanted_handles(), &HashSet::from([h(2)]));
        let (t, counts) = join.finish(&HashMap::from([(h(1), "Hat".to_owned())]));
        assert_eq!(t.lookup_item("ARM_Dup"), None, "earlier name must not survive");
        assert_eq!((counts.items, counts.skipped), (0, 1));
    }
}

rl_test! {
    fn stats_row_cap_at_edge_and_edge_plus_one() {
        let entries = |r: std::ops::Range<usize>| r.map(|i| format!("new entry \"S{i}\"\n")).collect::<String>();
        let mut b = CatalogueBuilder::new();
        let half = CAT_MAX_ROWS / 2; // a file holds at most STATS_MAX_ENTRIES
        b.add_stats(&entries(0..half), &mut never()).unwrap();
        b.add_stats(&entries(half..CAT_MAX_ROWS), &mut never()).unwrap();
        let more = entries(CAT_MAX_ROWS..CAT_MAX_ROWS + 1);
        let (what, size, limit) = too_large(b.add_stats(&more, &mut never()));
        assert_eq!((what, size, limit), ("stats entries", 200_001, 200_000));
    }
}

rl_test! {
    fn lsx_row_cap_at_edge_and_edge_plus_one() {
        let file = |r: std::ops::Range<usize>| {
            let rows: Vec<(String, String)> = r.map(|i| (format!("O{i}"), h(i as u32))).collect();
            let refs: Vec<(&str, &str)> = rows.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            lsx("Origin", "Name", "DisplayName", &refs)
        };
        let mut b = CatalogueBuilder::new();
        for start in (0..CAT_MAX_ROWS).step_by(50_000) {
            b.add_lsx(EntryKind::Origins, &file(start..start + 50_000)).unwrap();
        }
        let (what, size, limit) = too_large(b.add_lsx(EntryKind::Origins, &file(CAT_MAX_ROWS..CAT_MAX_ROWS + 1)));
        assert_eq!((what, size, limit), ("LSX ids", 200_001, 200_000));
    }
}
