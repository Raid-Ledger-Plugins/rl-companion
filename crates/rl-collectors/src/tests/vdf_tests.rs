//! Valve KeyValues parser (ROK-2081). Text fixtures are hand-written in the
//! shape Steam writes; paths and ids are invented.

use crate::{
    AppManifest, SteamLibrary, VDF_MAX_BYTES, VDF_MAX_DEPTH, VdfError, VdfValue, app_manifest,
    library_paths, parse_vdf,
};

const LIBRARY_FOLDERS: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"contentid"		"1111111111111111111"
		"totalsize"		"0"
		"apps"
		{
			"228980"		"123456"
		}
	}
	// a comment Steam never writes, but users do
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"apps"
		{
			"1086940"		"150000000000"
			"250820"		"1"
		}
	}
}
"#;

const APP_MANIFEST: &str = r#"
"AppState"
{
	"appid"		"1086940"
	"Universe"		"1"
	"name"		"Baldur's Gate 3"
	"StateFlags"		"4"
	"installdir"		"Baldurs Gate 3"
	"buildid"		"12345678"
	"InstalledDepots"
	{
		"1419651"
		{
			"manifest"		"1111111111111111111"
			"size"		"100"
		}
	}
	"UserConfig"
	{
		"language"		"english"
	}
}
"#;

fn manifest_with_installdir(dir: &str) -> String {
    format!("\"AppState\" {{ \"installdir\" \"{dir}\" }}")
}

fn nested(depth: usize) -> String {
    format!(
        "{}\"k\" \"v\"{}",
        "\"o\" { ".repeat(depth),
        " }".repeat(depth)
    )
}

rl_test! {
    fn reads_both_libraries_and_their_apps_with_windows_paths_unescaped() {
        let libs = library_paths(LIBRARY_FOLDERS).expect("libraryfolders.vdf");
        assert_eq!(libs, vec![
            SteamLibrary { path: r"C:\Program Files (x86)\Steam".into(), apps: vec!["228980".into()] },
            SteamLibrary { path: r"D:\SteamLibrary".into(), apps: vec!["1086940".into(), "250820".into()] },
        ]);
    }
}

rl_test! {
    fn reads_the_pre_2021_library_format_and_skips_non_library_keys() {
        let text = r#""LibraryFolders" { "TimeNextStatsReport" "1" "ContentStatsID" "2" "1" "E:\\Games\\Steam" "2" "" }"#;
        assert_eq!(library_paths(text).expect("old format"), vec![SteamLibrary { path: r"E:\Games\Steam".into(), apps: vec![] }]);
        assert_eq!(library_paths(r#""other" { }"#), Err(VdfError::MissingKey("libraryfolders")));
    }
}

rl_test! {
    fn reads_an_app_manifest() {
        assert_eq!(app_manifest(APP_MANIFEST).expect("acf"), AppManifest {
            appid: Some("1086940".into()),
            installdir: "Baldurs Gate 3".into(),
            buildid: Some("12345678".into()),
        });
        let minimal = app_manifest(r#""appstate" { "InstallDir" "BG3" "buildid" "" }"#).expect("keys are case-insensitive");
        assert_eq!((minimal.installdir.as_str(), minimal.appid, minimal.buildid), ("BG3", None, None));
    }
}

rl_test! {
    fn refuses_an_installdir_that_is_not_one_folder_name() {
        for dir in ["", ".", "..", "a/b", r"a\\b", "C:x", "../escape"] {
            assert_eq!(
                app_manifest(&manifest_with_installdir(dir)),
                Err(VdfError::BadValue { key: "installdir", reason: "not a single folder name" }),
                "installdir {dir:?}"
            );
        }
        assert_eq!(app_manifest(r#""AppState" { "appid" "1" }"#), Err(VdfError::MissingKey("installdir")));
        assert_eq!(app_manifest(r#""AppState" { "installdir" { } }"#), Err(VdfError::MissingKey("installdir")));
        assert_eq!(app_manifest(r#""Other" { }"#), Err(VdfError::MissingKey("AppState")));
    }
}

rl_test! {
    fn nesting_is_capped_at_the_edge() {
        assert_eq!(VDF_MAX_DEPTH, 8);
        assert!(parse_vdf(&nested(VDF_MAX_DEPTH)).is_ok(), "{VDF_MAX_DEPTH} levels must parse");
        assert_eq!(parse_vdf(&nested(VDF_MAX_DEPTH + 1)), Err(VdfError::TooDeep { line: 1, limit: VDF_MAX_DEPTH }));
    }
}

rl_test! {
    fn input_size_is_capped_at_the_edge() {
        let pair = "\"k\" \"v\"";
        let at_cap = format!("{pair}{}", " ".repeat(VDF_MAX_BYTES - pair.len()));
        assert_eq!(at_cap.len(), VDF_MAX_BYTES);
        assert_eq!(parse_vdf(&at_cap).expect("exactly the cap").str("k"), Some("v"));
        let over = format!("{at_cap} ");
        assert_eq!(parse_vdf(&over), Err(VdfError::TooLarge { size: VDF_MAX_BYTES + 1, limit: VDF_MAX_BYTES }));
    }
}

rl_test! {
    fn malformed_input_is_a_typed_error_with_its_line() {
        let cases: [(&str, usize, &str); 6] = [
            ("\"a\" \"unterminated", 1, "unterminated string"),
            ("\"a\" \"b\"\n}", 2, "unmatched '}'"),
            ("\"a\"", 1, "key without a value"),
            ("\"a\" { \"b\" }", 1, "key without a value"),
            ("\"a\" {\n\"b\" \"c\"\n", 3, "unterminated object"),
            ("{ }", 1, "'{' where a key was expected"),
        ];
        for (text, line, reason) in cases {
            assert_eq!(parse_vdf(text), Err(VdfError::Syntax { line, reason }), "input {text:?}");
        }
    }
}

rl_test! {
    fn escapes_bare_tokens_conditionals_and_duplicates() {
        let doc = parse_vdf(
            "\"esc\" \"q\\\"t\\tn\\nb\\\\ \\x kept\"\nbare value [$WIN32]\n\"dup\" \"first\" \"DUP\" \"second\"\n\"multi\nline\" \"ok\"",
        ).expect("valid");
        assert_eq!(doc.str("esc"), Some("q\"t\tn\nb\\ \\x kept"));
        assert_eq!(doc.str("bare"), Some("value"), "a bare token is a string and the conditional is dropped");
        assert_eq!(doc.str("dup"), Some("first"), "lookup returns the first of duplicate keys, ignoring case");
        assert_eq!(doc.0.len(), 5, "duplicate keys are kept in file order");
        assert_eq!(doc.str("multi\nline"), Some("ok"));
        assert_eq!(doc.get("esc").map(|v| matches!(v, VdfValue::Str(_))), Some(true));
        assert_eq!(doc.obj("esc"), None);
    }
}

rl_test! {
    fn empty_and_comment_only_input_is_an_empty_object() {
        assert_eq!(parse_vdf("").expect("empty").0.len(), 0);
        assert_eq!(parse_vdf("// only a comment\n   ").expect("comment").0.len(), 0);
    }
}
