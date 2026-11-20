//! BugGrabber addon-errors core (ROK-2099 S5). The fixture is a synthetic
//! `!BugGrabber.lua` (placeholder names `Testchar` / `Example Realm` /
//! `ExampleUser`, invented messages), compiled in with `include_bytes!`.

use crate::{
    ADDON_ERRORS_MAX_BYTES, AddonErrorRecord, AddonErrorsMessage, AddonErrorsState, CharacterHint,
    PayloadHeader, ScrubContext, TRUNCATED_MARKER, TimeContext, account_characters,
    account_sv_candidates, build_messages, has_unscrubbed_home, has_unscrubbed_path,
    hmac_sha256_hex, scan_buggrabber, scrub_battletags, scrub_names, scrub_paths, sha256_hex,
    toc_version,
};

pub(crate) const BUGGRABBER: &[u8] =
    include_bytes!("../../tests/fixtures/addon_errors/buggrabber.lua");

pub(crate) fn ctx() -> ScrubContext {
    ScrubContext {
        characters: vec!["Testchar".into()],
        realms: vec!["Example Realm".into()],
        battle_tag: None,
        account: Some("ACCT1".into()),
    }
}

pub(crate) const UTC: TimeContext = TimeContext {
    utc_offset_seconds: 0,
    fallback_unix: 1_790_000_000,
};

fn by_addon<'a>(records: &'a [AddonErrorRecord], addon: &str) -> &'a AddonErrorRecord {
    records.iter().find(|r| r.addon == addon).expect(addon)
}

rl_test! {
    fn parses_merges_and_never_reads_locals() {
        let scan = scan_buggrabber(BUGGRABBER, &ctx(), &UTC).expect("parses");
        assert_eq!(scan.records.len(), 3, "{:#?}", scan.records);
        assert_eq!(scan.skipped, 2, "a string entry and an entry without message");
        let questie = by_addon(&scan.records, "Questie");
        assert_eq!(questie.count, 5, "two entries scrub to one record");
        assert_eq!(questie.first_seen_at, "2026-11-16T07:00:00Z");
        assert_eq!(questie.last_seen_at, "2026-11-17T08:00:00Z");
        assert_eq!(questie.session, Some(41));
        assert_eq!(by_addon(&scan.records, "OpenMap").count, 1, "BugGrabber itself is skipped");
        let json = serde_json::to_string(&scan.records).expect("serialises");
        assert!(!json.contains("Otherplayer") && !json.contains("locals"), "{json}");
        assert_eq!(scan.file_sha256, sha256_hex(BUGGRABBER));
    }
}

rl_test! {
    fn scrubs_paths_and_own_names_in_the_fixture() {
        let scan = scan_buggrabber(BUGGRABBER, &ctx(), &UTC).expect("parses");
        let buff = by_addon(&scan.records, "BuffRadar");
        assert_eq!(buff.message, r"<wow>\Interface\AddOns\BuffRadar\Core.lua:5: <char> of <realm>: bad argument");
        let stack = buff.stack.as_deref().expect("stack");
        assert!(stack.starts_with(r"<wow>\Interface/AddOns/BuffRadar/Core.lua:5"), "{stack}");
        assert!(stack.contains("\n<path>\\notes.txt\n"), "{stack}");
        assert!(stack.contains(r"WTF\Account\<account>\SavedVariables\BuffRadar.lua"), "{stack}");
        assert!(stack.ends_with("<char>-<realm>"), "{stack}");
        for r in &scan.records {
            for text in [Some(r.message.as_str()), r.stack.as_deref()].into_iter().flatten() {
                assert!(!has_unscrubbed_path(text), "{text}");
                assert!(!text.contains("ExampleUser") && !text.to_lowercase().contains("exampleuser"), "{text}");
                assert!(!text.contains("Testchar") && !text.contains("Example Realm"), "{text}");
                assert!(!text.contains("ACCT1"), "{text}");
            }
        }
    }
}

rl_test! {
    fn scrubs_windows_and_macos_home_folders() {
        assert_eq!(scrub_paths(r"at C:\Users\ExampleUser\Documents\x.lua:3"), r"at <home>\Documents\x.lua:3");
        assert_eq!(scrub_paths("at /Users/exampleuser/Desktop/x.lua"), r"at <home>\Desktop/x.lua");
        assert_eq!(scrub_paths(r"c:/users/Example User/a"), r"<home>\a");
        assert_eq!(
            scrub_paths(r"C:\Program Files (x86)\World of Warcraft\_retail_\Interface\AddOns\A\b.lua:1"),
            r"<wow>\Interface\AddOns\A\b.lua:1"
        );
        assert_eq!(scrub_paths("see https://example.com/x"), "see https://example.com/x");
        assert_eq!(scrub_paths(r"Interface\AddOns\A\b.lua:1"), r"Interface\AddOns\A\b.lua:1");
    }
}

rl_test! {
    fn scrubbed_text_is_never_refused_by_the_contract_pattern() {
        let nasty = [
            r"C:\Users\", r"x\USERS\y", "/Users", "/users/", r"A:\", "(Z:/tmp)", r"\Users\\", "é:/",
            r"D:\Games\World of Warcraft\_classic_\", "C:/C:/C:/", r"1C:\x", "/Users/a/Users/b/",
            "\u{e9}C:/x", r"C:\Users\ExampleUser", "World of Warcraft/_x_/C:\\Users\\a\\",
        ];
        for s in nasty {
            let out = scrub_paths(s);
            assert!(!has_unscrubbed_path(&out), "{s:?} -> {out:?}");
        }
        assert!(has_unscrubbed_path(r"C:\x") && has_unscrubbed_path("/Users/x/"));
        assert!(!has_unscrubbed_path("https://x") && !has_unscrubbed_path(r"<home>\x"));
    }
}

rl_test! {
    fn replaces_whole_word_names_only() {
        let names = vec![("Testchar".to_owned(), "<char>"), ("Name#1234".to_owned(), "<char>")];
        assert_eq!(scrub_names("Testchar, TESTCHAR and Testcharlike", &names), "<char>, <char> and Testcharlike");
        assert_eq!(scrub_names("tag Name#1234!", &names), "tag <char>!");
    }
}

rl_test! {
    fn applies_the_local_utc_offset_and_the_fallback() {
        let plus_one = TimeContext { utc_offset_seconds: 3600, ..UTC };
        let scan = scan_buggrabber(BUGGRABBER, &ctx(), &plus_one).expect("parses");
        assert_eq!(by_addon(&scan.records, "OpenMap").last_seen_at, "2026-11-18T20:31:00Z");
        let no_time = b"BugGrabberDB = { errors = { { message = \"x\", counter = 0 } } }";
        let scan = scan_buggrabber(no_time, &ScrubContext::default(), &UTC).expect("parses");
        let [r] = scan.records.as_slice() else { panic!("one record") };
        assert_eq!((r.first_seen_at.as_str(), r.count, r.addon.as_str()), ("2026-09-21T14:13:20Z", 1, "Unknown"));
    }
}

rl_test! {
    fn files_without_errors_yield_nothing() {
        for src in [&b"Other = {}"[..], b"BugGrabberDB = { session = 1 }", b""] {
            let scan = scan_buggrabber(src, &ctx(), &UTC).expect("parses");
            assert!(scan.records.is_empty());
        }
        assert!(scan_buggrabber(b"BugGrabberDB = f()", &ctx(), &UTC).is_err());
    }
}

rl_test! {
    fn truncates_long_stacks_with_the_marker() {
        let long = format!("BugGrabberDB = {{ errors = {{ {{ message = \"m\", stack = \"{}\" }} }} }}", "a".repeat(5000));
        let scan = scan_buggrabber(long.as_bytes(), &ctx(), &UTC).expect("parses");
        let stack = scan.records[0].stack.as_deref().expect("stack");
        assert_eq!(stack.encode_utf16().count(), 4096);
        assert!(stack.ends_with(TRUNCATED_MARKER));
    }
}

rl_test! {
    fn an_unchanged_rescan_yields_nothing_and_only_new_errors_follow() {
        let scan = scan_buggrabber(BUGGRABBER, &ctx(), &UTC).expect("parses");
        let mut state = AddonErrorsState::default();
        let first = state.pending(&scan);
        assert_eq!(first.len(), 3);
        state.mark_uploaded(&first);
        state.finish_scan(&scan);
        let again = scan_buggrabber(BUGGRABBER, &ctx(), &UTC).expect("parses");
        assert!(state.is_unchanged(&again) && state.pending(&again).is_empty());
        let grown = String::from_utf8_lossy(BUGGRABBER).replace("[\"counter\"] = 3", "[\"counter\"] = 4");
        let next = scan_buggrabber(grown.as_bytes(), &ctx(), &UTC).expect("parses");
        let pending = state.pending(&next);
        assert_eq!(pending.iter().map(|r| (r.addon.as_str(), r.count)).collect::<Vec<_>>(), [("Questie", 6)]);
        let json = serde_json::to_string(&state).expect("state serialises");
        assert_eq!(serde_json::from_str::<AddonErrorsState>(&json).expect("round trip"), state);
    }
}

/// A fixed per-install key for tests (production: 32 CSPRNG bytes on disk).
pub(crate) const INSTALL_KEY: [u8; 32] = [7u8; 32];

pub(crate) fn header() -> PayloadHeader {
    let hint = CharacterHint {
        name: "Testchar".into(),
        realm: "Example Realm".into(),
    };
    PayloadHeader::new(
        "12.1.1",
        "EXAMPLEACCOUNT",
        &INSTALL_KEY,
        Some(hint),
        Some("_classic_era_"),
    )
    .expect("valid header")
}

fn record(i: usize, message: &str, stack: Option<String>) -> AddonErrorRecord {
    AddonErrorRecord {
        addon: format!("TestAddon{i:02}"),
        message: format!("{message} {i}"),
        stack,
        first_seen_at: "2026-11-17T08:00:00Z".into(),
        last_seen_at: "2026-11-17T08:00:00Z".into(),
        count: 1,
        session: None,
    }
}

rl_test! {
    fn splits_batches_by_count_and_by_size() {
        let small: Vec<_> = (0..120).map(|i| record(i, "boom", None)).collect();
        let msgs = build_messages(&header(), small, "2026-11-19T12:00:05Z", "0.4.0").expect("builds");
        assert_eq!(msgs.iter().map(|m| m.payload.records.len()).collect::<Vec<_>>(), [50, 50, 20]);
        let big: Vec<_> = (0..50).map(|i| record(i, &"€".repeat(1000), Some("€".repeat(4096)))).collect();
        let msgs = build_messages(&header(), big, "2026-11-19T12:00:05Z", "0.4.0").expect("builds");
        assert!(msgs.len() > 1, "a 50 x 15 KB batch must split");
        assert_eq!(msgs.iter().map(|m| m.payload.records.len()).sum::<usize>(), 50);
        for m in &msgs {
            assert!(serde_json::to_vec(m).expect("json").len() <= ADDON_ERRORS_MAX_BYTES);
        }
        assert!(build_messages(&header(), Vec::new(), "2026-11-19T12:00:05Z", "0.4.0").expect("ok").is_empty());
    }
}

rl_test! {
    fn validates_the_header() {
        assert!(PayloadHeader::new(r"C:\Users\x", "A", &INSTALL_KEY, None, None).is_err());
        assert!(PayloadHeader::new("1.0", "A", &INSTALL_KEY, None, Some("retail")).is_err());
        assert!(PayloadHeader::new("1.0", "", &INSTALL_KEY, None, None).is_err());
        assert_eq!(toc_version(b"## Title: BugGrabber\r\n## Version: 12.1.1\r\n").as_deref(), Some("12.1.1"));
        assert_eq!(toc_version(b"## Title: x"), None);
    }
}

rl_test! {
    fn finds_buggrabber_files_and_account_characters() {
        let listing = [
            "_retail_/WTF/Account/ACCT1/SavedVariables/!BugGrabber.lua",
            "_retail_/WTF/Account/ACCT1/SavedVariables/LedgerLink.lua",
            "_retail_/WTF/Account/ACCT1/Example Realm/Testchar/SavedVariables/!BugGrabber.lua",
            "_retail_/WTF/Account/ACCT1/Example Realm/Otheralt/config-cache.wtf",
            "_retail_/WTF/Account/ACCT2/Other Realm/Elsewhere/config-cache.wtf",
        ];
        let found = account_sv_candidates("/wow", listing, "!BugGrabber.lua");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "/wow/_retail_/WTF/Account/ACCT1/SavedVariables/!BugGrabber.lua");
        let chars = account_characters("/wow", listing, "_retail_", "ACCT1");
        assert_eq!(chars, [("Example Realm".to_owned(), "Otheralt".to_owned()), ("Example Realm".to_owned(), "Testchar".to_owned())]);
    }
}

rl_test! {
    fn scrubs_the_account_folder_name_and_its_number() {
        let lua = br#"BugGrabberDB = { ["errors"] = { { ["message"] = "WTF\\Account\\12345678#1\\SavedVariables\\A.lua:1: 12345678 failed", ["counter"] = 1 } } }"#;
        let ctx = ScrubContext { account: Some("12345678#1".into()), ..ctx() };
        let scan = scan_buggrabber(lua, &ctx, &UTC).expect("parses");
        let r = scan.records.first().expect("one record");
        assert_eq!(r.message, r"WTF\Account\<account>\SavedVariables\A.lua:1: <account> failed");
    }
}

rl_test! {
    fn scrubs_wine_linux_unc_and_other_drive_paths() {
        assert_eq!(scrub_paths(r"at Z:\home\bob\Games\x.lua:3"), r"at <home>\Games\x.lua:3");
        assert_eq!(scrub_paths("at /home/bob/.wine/x.lua:3"), r"at <home>\.wine/x.lua:3");
        assert_eq!(scrub_paths("/home/bob/World of Warcraft/_retail_/Interface/AddOns/X/a.lua:1"), r"<wow>\Interface/AddOns/X/a.lua:1");
        assert_eq!(scrub_paths(r"\\NAS\bob\World of Warcraft\_retail_\Interface\AddOns\X\a.lua:1"), r"<wow>\Interface\AddOns\X\a.lua:1");
        assert_eq!(scrub_paths(r"see \\NAS\bob\notes.txt:2"), r"see <path>\notes.txt:2");
        assert_eq!(scrub_paths(r"D:\Bob\Games\x.lua:3: boom"), r"<path>\x.lua:3: boom");
        assert_eq!(scrub_paths(r"D:\Bob\Interface\AddOns\X\a.lua:1"), r"<path>\Interface\AddOns\X\a.lua:1");
        assert_eq!(scrub_paths(r"Interface\AddOns\Home\home\x.lua:1"), r"Interface\AddOns\Home\home\x.lua:1");
    }
}

rl_test! {
    fn masks_memory_addresses_so_a_varying_error_keeps_one_key() {
        let lua = br#"BugGrabberDB = { ["errors"] = { { ["message"] = "Interface\\AddOns\\Foo\\F.lua:1: bad table: 000001F3A2B4C5D0", ["counter"] = 1 }, { ["message"] = "Interface\\AddOns\\Foo\\F.lua:1: bad table: 0x7ffd12a4", ["counter"] = 2 } } }"#;
        let scan = scan_buggrabber(lua, &ctx(), &UTC).expect("parses");
        assert_eq!(scan.records.len(), 1, "{:#?}", scan.records);
        let r = scan.records.first().expect("one record");
        assert_eq!(r.message, r"Interface\AddOns\Foo\F.lua:1: bad table: <addr>");
        assert_eq!(r.count, 3);
    }
}

rl_test! {
    fn fifty_records_stay_one_message_and_fifty_one_split() {
        for (n, parts) in [(50, vec![50]), (51, vec![50, 1])] {
            let rs: Vec<_> = (0..n).map(|i| record(i, "boom", None)).collect();
            let msgs = build_messages(&header(), rs, "2026-11-19T12:00:05Z", "0.4.0").expect("builds");
            assert_eq!(msgs.iter().map(|m| m.payload.records.len()).collect::<Vec<_>>(), parts);
        }
    }
}

rl_test! {
    fn a_batch_of_exactly_the_byte_cap_stays_one_message() {
        let msgs = |n: usize, extra: usize| {
            let mut rs: Vec<_> = (0..n).map(|i| record(i, "boom", Some("€".repeat(2000)))).collect();
            let stack = format!("a{}{}", "€".repeat(extra / 3), "a".repeat(extra % 3));
            rs.push(record(n, "edge", Some(stack)));
            build_messages(&header(), rs, "2026-11-19T12:00:05Z", "0.4.0").expect("builds")
        };
        let size = |ms: &[AddonErrorsMessage]| serde_json::to_vec(ms.first().expect("one")).expect("json").len();
        let mut n = 0;
        while size(&msgs(n + 1, 0)) <= ADDON_ERRORS_MAX_BYTES && msgs(n + 1, 0).len() == 1 {
            n += 1;
        }
        let rem = ADDON_ERRORS_MAX_BYTES - size(&msgs(n, 0));
        assert!(rem > 1 && rem < 7000, "filler fits one stack: {rem}");
        let under = msgs(n, rem - 1);
        assert_eq!((under.len(), size(&under)), (1, ADDON_ERRORS_MAX_BYTES - 1));
        let exact = msgs(n, rem);
        assert_eq!((exact.len(), size(&exact)), (1, ADDON_ERRORS_MAX_BYTES), "equal to the cap stays one message");
        let over = msgs(n, rem + 1);
        assert_eq!(over.len(), 2, "one byte over the cap splits");
        assert!(over.iter().all(|m| serde_json::to_vec(m).expect("json").len() <= ADDON_ERRORS_MAX_BYTES));
    }
}

rl_test! {
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(mac, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }
}

rl_test! {
    fn account_key_is_an_install_keyed_hmac_not_a_bare_hash() {
        let key = |folder: &str, install: &[u8; 32]| {
            PayloadHeader::new("1.0", folder, install, None, None).expect("valid").account_key
        };
        let a = key("12345678#1", &INSTALL_KEY);
        assert_eq!(a, key("12345678#1", &INSTALL_KEY), "same folder + same key => same value");
        assert_ne!(a, key("12345678#1", &[8u8; 32]), "another install => another value");
        assert_ne!(a, key("12345678#2", &INSTALL_KEY));
        assert_ne!(a, sha256_hex(b"12345678#1"), "never the bare SHA-256 of the folder");
        assert_eq!(a, hmac_sha256_hex(&INSTALL_KEY, b"12345678#1"));
        assert!(a.len() == 64 && a.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    }
}

rl_test! {
    fn scrubs_doubled_escaped_and_mid_string_home_folders() {
        let cases = [
            (r"x\\Users\\bob\\a.lua:1", r"x\<home>\a.lua:1"),
            (r"at \/Users\/bob\/a.lua:1", r"at \<home>\a.lua:1"),
            (r"C:\\Users\\bob\\a.lua:1", r"<home>\a.lua:1"),
            ("file:/home/bob/a.lua:1", r"file:<home>\a.lua:1"),
            ("x,/home/bob/a.lua:1", r"x,<home>\a.lua:1"),
            ("x//home//bob//a.lua:1", r"x/<home>\a.lua:1"),
            (r"{\\NAS\bob\notes.txt:2", r"{<path>\notes.txt:2"),
            (r"x,\\NAS\bob\notes.txt:2", r"x,<path>\notes.txt:2"),
            (r"Interface\AddOns\Home\home\x.lua:1", r"Interface\AddOns\Home\home\x.lua:1"),
        ];
        for (raw, want) in cases {
            let got = scrub_paths(raw);
            assert_eq!(got, want, "{raw}");
            assert!(!got.contains("bob") && !has_unscrubbed_home(&got), "{raw} -> {got}");
        }
        assert!(has_unscrubbed_home("x,/home/bob/") && has_unscrubbed_home(r"{\\NAS\x"));
    }
}

rl_test! {
    fn mid_string_homes_never_reach_message_stack_or_addon() {
        let lua = br#"BugGrabberDB = { ["errors"] = { { ["message"] = "file:/home/bob/x\\Users\\bob\\Interface\\AddOns\\Foo\\F.lua:1: {\\\\NAS\\bob\\n.txt", ["stack"] = "x,/home/bob/a.lua:2: in main\n\\/Users\\/bob\\/b.lua:3", ["counter"] = 1 } } }"#;
        let scan = scan_buggrabber(lua, &ctx(), &UTC).expect("parses");
        let r = scan.records.first().expect("one record");
        let all = format!("{} {} {:?}", r.addon, r.message, r.stack);
        assert!(!all.contains("bob"), "{all}");
    }
}

rl_test! {
    fn scrubs_battletags_but_not_ids_or_colours() {
        let cases = [
            ("whisper from Bob#1234 failed", "whisper from <battletag> failed"),
            ("Ünïcødé#123456: x", "<battletag>: x"),
            ("(Ab1#12345)", "(<battletag>)"),
            ("quest #123 and item#12", "quest #123 and item#12"),
            ("|cff00ff00Item|r #45678", "|cff00ff00Item|r #45678"),
            ("Bob#1234567 Bob#123 1Bob#1234 TwelveLetters#1234", "Bob#1234567 Bob#123 1Bob#1234 TwelveLetters#1234"),
        ];
        for (raw, want) in cases {
            assert_eq!(scrub_battletags(raw), want, "{raw}");
        }
    }
}

rl_test! {
    fn the_account_folder_scrub_wins_over_the_battletag_scrub() {
        let lua = br#"BugGrabberDB = { ["errors"] = { { ["message"] = "WTF\\Account\\Legacy#1234\\SavedVariables\\A.lua:1: Bob#5678 quest #123", ["stack"] = "from Bob#5678", ["counter"] = 1 } } }"#;
        let ctx = ScrubContext { account: Some("Legacy#1234".into()), ..ctx() };
        let scan = scan_buggrabber(lua, &ctx, &UTC).expect("parses");
        let r = scan.records.first().expect("one record");
        assert_eq!(r.message, r"WTF\Account\<account>\SavedVariables\A.lua:1: <battletag> quest #123");
        assert_eq!(r.stack.as_deref(), Some("from <battletag>"));
    }
}
