//! `.loca` reader, entry classifier and catalogue tables (ROK-2081).
//! Files are built by `loca()` below with invented handles; no game bytes.

use std::collections::HashSet;

use crate::{
    CatalogueError, CatalogueTables, EntryKind, ItemName, LOCA_ENTRY_BYTES, LOCA_HEADER_BYTES,
    LOCA_KEY_BYTES, LOCA_MAX_ENTRIES, classify_entry, is_loca_handle, parse_loca_wanted,
};

/// An invented, well-formed handle.
pub(super) fn h(n: u32) -> String {
    format!("h{n:08x}g0000g0000g0000g000000000000")
}

/// A `.loca` file: each text gets its trailing NUL.
pub(super) fn loca(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let texts_at = LOCA_HEADER_BYTES + entries.len() * LOCA_ENTRY_BYTES;
    let mut out = b"LOCA".to_vec();
    out.extend((entries.len() as u32).to_le_bytes());
    out.extend((texts_at as u32).to_le_bytes());
    for (key, text) in entries {
        let mut field = key.to_vec();
        field.resize(LOCA_KEY_BYTES, 0);
        out.extend(field);
        out.extend(1u16.to_le_bytes());
        out.extend((text.len() as u32 + 1).to_le_bytes());
    }
    for (_, text) in entries {
        out.extend(*text);
        out.push(0);
    }
    out
}

fn wanted(keys: &[&str]) -> HashSet<String> {
    keys.iter().map(|k| (*k).to_owned()).collect()
}

fn parse_reason(r: Result<impl std::fmt::Debug, CatalogueError>) -> &'static str {
    match r {
        Err(CatalogueError::Parse { reason, .. }) => reason,
        other => panic!("expected a Parse error, got {other:?}"),
    }
}

rl_test! {
    fn loca_keeps_only_wanted_handles_that_exist() {
        let (a, b, c) = (h(1), h(2), h(3));
        let bytes = loca(&[(a.as_bytes(), b"Hat"), (b.as_bytes(), b"Sword"), (c.as_bytes(), b"Boots")]);
        let got = parse_loca_wanted(&bytes, &wanted(&[&a, &c, &h(9)])).unwrap();
        assert_eq!(got.len(), 2, "only the two wanted handles present: {got:?}");
        assert_eq!(got.get(&a).map(String::as_str), Some("Hat"));
        assert_eq!(got.get(&c).map(String::as_str), Some("Boots"));
        assert!(!got.contains_key(&b), "unwanted handle kept");
        assert!(!got.contains_key(&h(9)), "missing handle invented");
    }
}

rl_test! {
    fn loca_non_utf8_text_or_key_drops_only_that_entry() {
        let (a, b) = (h(1), h(2));
        let bad_key: &[u8] = &[0xff, 0xfe, b'x'];
        let bytes = loca(&[(a.as_bytes(), &[0xc3, 0x28]), (bad_key, b"Ghost"), (b.as_bytes(), b"Caf\xc3\xa9")]);
        let got = parse_loca_wanted(&bytes, &wanted(&[&a, &b])).unwrap();
        assert_eq!(got.get(&a), None, "non-UTF-8 text must drop the entry");
        assert_eq!(got.get(&b).map(String::as_str), Some("Café"), "later entries still read");
    }
}

rl_test! {
    fn loca_later_duplicate_wins_and_text_without_nul_is_kept_whole() {
        let a = h(1);
        let mut bytes = loca(&[(a.as_bytes(), b"Old"), (a.as_bytes(), b"New")]);
        assert_eq!(parse_loca_wanted(&bytes, &wanted(&[&a])).unwrap()[&a], "New");
        // Last text's NUL removed and its length shrunk: still read whole.
        bytes.pop();
        let len_at = LOCA_HEADER_BYTES + LOCA_ENTRY_BYTES + LOCA_KEY_BYTES + 2;
        bytes[len_at..len_at + 4].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(parse_loca_wanted(&bytes, &wanted(&[&a])).unwrap()[&a], "New");
    }
}

rl_test! {
    fn loca_text_ending_exactly_at_eof_reads_and_one_byte_past_errors() {
        let a = h(1);
        let mut bytes = loca(&[(a.as_bytes(), b"Hat")]);
        assert_eq!(parse_loca_wanted(&bytes, &wanted(&[&a])).unwrap()[&a], "Hat");
        bytes.pop();
        let r = parse_loca_wanted(&bytes, &wanted(&[&a]));
        assert_eq!(parse_reason(r), "text runs past the end of the file");
    }
}

rl_test! {
    fn loca_header_and_table_errors() {
        assert_eq!(parse_reason(parse_loca_wanted(b"LOCA\0\0\0", &wanted(&[]))), "header truncated");
        let mut bytes = loca(&[(h(1).as_bytes(), b"Hat")]);
        bytes[0] = b'X';
        assert_eq!(parse_reason(parse_loca_wanted(&bytes, &wanted(&[]))), "not a LOCA file");
        let mut bytes = loca(&[(h(1).as_bytes(), b"Hat")]);
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
        let r = parse_loca_wanted(&bytes, &wanted(&[]));
        assert_eq!(parse_reason(r), "entry table runs past the end");
        let mut bytes = loca(&[(h(1).as_bytes(), b"Hat")]);
        bytes[8..12].copy_from_slice(&11u32.to_le_bytes());
        assert_eq!(parse_reason(parse_loca_wanted(&bytes, &wanted(&[]))), "text offset outside the file");
    }
}

rl_test! {
    fn loca_entry_count_cap_at_edge_and_edge_plus_one() {
        // A header alone: at the cap the count is accepted (and the table
        // check fires); one over, the cap itself refuses it.
        let header = |count: u32| {
            let mut b = b"LOCA".to_vec();
            b.extend(count.to_le_bytes());
            b.extend(12u32.to_le_bytes());
            b
        };
        let at_cap = parse_loca_wanted(&header(LOCA_MAX_ENTRIES), &wanted(&[]));
        assert_eq!(parse_reason(at_cap), "entry table runs past the end");
        assert_eq!(
            parse_loca_wanted(&header(LOCA_MAX_ENTRIES + 1), &wanted(&[])),
            Err(CatalogueError::TooLarge {
                what: "loca entry count",
                size: u64::from(LOCA_MAX_ENTRIES) + 1,
                limit: u64::from(LOCA_MAX_ENTRIES),
            })
        );
    }
}

rl_test! {
    fn loca_handle_shape() {
        assert!(is_loca_handle(&h(0xdead_beef)));
        assert!(is_loca_handle("hABCDEF01g1234g5678g9ABCg0123456789AB"));
        assert!(!is_loca_handle(&h(1)[..36]), "too short");
        assert!(!is_loca_handle(&format!("{}0", h(1))), "too long");
        assert!(!is_loca_handle("x00000001g0000g0000g0000g000000000000"), "no leading h");
        assert!(!is_loca_handle("h00000001-0000g0000g0000g000000000000"), "bad separator");
        assert!(!is_loca_handle("h0000000zg0000g0000g0000g000000000000"), "non-hex");
    }
}

rl_test! {
    fn classify_entry_matches_each_glob_and_nothing_else() {
        let cases = [
            ("Localization/English/english.loca", Some(EntryKind::Loca)),
            ("Public/Shared/Stats/Generated/Data/Armor.txt", Some(EntryKind::StatsTxt)),
            ("Public/Gustav/Stats/Generated/Data/Weapon.txt", Some(EntryKind::StatsTxt)),
            ("Public/GustavDev/Stats/Generated/Data/Object.txt", Some(EntryKind::StatsTxt)),
            ("Public/Shared/RootTemplates/_merged.lsf", Some(EntryKind::RootTemplates)),
            ("Public/Gustav/Origins/Origins.lsx", Some(EntryKind::Origins)),
            ("Mods/Gustav/Story/Journal/quest_prototypes.lsx", Some(EntryKind::Quests)),
            ("public\\shared\\stats\\generated\\data\\ARMOR.TXT", Some(EntryKind::StatsTxt)),
            ("/Localization/English/english.loca", Some(EntryKind::Loca)),
            ("Localization/French/french.loca", None),
            ("Public/Shared/Stats/Generated/Data/Spell_Target.txt", None),
            ("Public/A/B/Stats/Generated/Data/Armor.txt", None),
            ("Public//Stats/Generated/Data/Armor.txt", None),
            ("Public/Shared/Stats/Generated/Data/Armor.txt.bak", None),
        ];
        for (path, want) in cases {
            assert_eq!(classify_entry(path), want, "path {path:?}");
        }
    }
}

rl_test! {
    fn catalogue_tables_lookup_and_round_trip_camel_case() {
        let mut t = CatalogueTables::default();
        let hat = ItemName { name: Some("Hat".into()), rarity: Some("Rare".into()) };
        t.items.insert("ARM_TestHat".into(), hat.clone());
        t.origins.insert("Tav".into(), "The Tav".into());
        t.quests.insert("Q_Test".into(), "A Quest".into());
        assert_eq!(t.lookup_item("ARM_TestHat"), Some(&hat));
        assert_eq!(t.lookup_item("ARM_Missing"), None);
        assert_eq!(t.lookup_origin("Tav"), Some("The Tav"));
        assert_eq!(t.lookup_quest("Q_Test"), Some("A Quest"));
        assert_eq!(t.lookup_waypoint("WP"), None);
        assert_eq!(t.row_count(), 3);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"waypoints\":{}"), "{json}");
        assert_eq!(serde_json::from_str::<CatalogueTables>(&json).unwrap(), t);
    }
}
