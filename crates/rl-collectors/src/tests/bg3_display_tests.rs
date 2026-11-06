//! D9 display filter for catalogue strings (ROK-2081). Invented text only.

use crate::{CAT_GUID_HEX_RUN, CAT_MAX_DIGIT_RUN, CAT_MAX_NAME_UTF16, catalogue_display_text};

rl_test! {
    fn display_strips_markup_and_collapses_whitespace() {
        let raw = "<LSTag Tooltip=\"x\">Test</LSTag>  Hat<br>of\t\nTesting ";
        assert_eq!(catalogue_display_text(raw).as_deref(), Some("Test Hat of Testing"));
        assert_eq!(catalogue_display_text("Plain Hat").as_deref(), Some("Plain Hat"));
    }
}

rl_test! {
    fn display_refuses_empty_tag_only_and_unclosed_markup() {
        for raw in ["", "   ", "<br><b></b>", "Hat <LSTag oops", "\u{2028}"] {
            assert_eq!(catalogue_display_text(raw), None, "{raw:?} should be refused");
        }
    }
}

rl_test! {
    fn display_name_length_at_edge_and_edge_plus_one_utf16_units() {
        let edge = "x".repeat(CAT_MAX_NAME_UTF16);
        assert_eq!(catalogue_display_text(&edge).as_deref(), Some(edge.as_str()));
        let over = "x".repeat(CAT_MAX_NAME_UTF16 + 1);
        assert_eq!(catalogue_display_text(&over), None, "129 units must be refused, not truncated");
        // A non-BMP char is two UTF-16 units.
        let astral_edge = format!("{}\u{1F600}", "x".repeat(CAT_MAX_NAME_UTF16 - 2));
        assert!(catalogue_display_text(&astral_edge).is_some(), "126 + 2 units fits");
        let astral_over = format!("{}\u{1F600}", "x".repeat(CAT_MAX_NAME_UTF16 - 1));
        assert_eq!(catalogue_display_text(&astral_over), None, "127 + 2 units is over");
    }
}

rl_test! {
    fn display_digit_run_at_edge_and_edge_plus_one() {
        let edge = format!("Ring {}", "7".repeat(CAT_MAX_DIGIT_RUN));
        assert_eq!(catalogue_display_text(&edge).as_deref(), Some(edge.as_str()));
        let over = format!("Ring {}", "7".repeat(CAT_MAX_DIGIT_RUN + 1));
        assert_eq!(catalogue_display_text(&over), None, "a 10-digit run is an id leak");
    }
}

rl_test! {
    fn display_refuses_guids_hyphenated_or_bare() {
        let hyphen = "Hat 0a1b2c3d-0000-4e5f-8a9b-0c1d2e3f4a5b";
        assert_eq!(catalogue_display_text(hyphen), None, "hyphenated GUID");
        let short = "Hat 0a1b2c3d-0000-4e5f-8a9b-0c1d2e3f4a5";
        assert!(catalogue_display_text(short).is_some(), "11-hex last group is not a GUID");
        let edge = format!("Hat {}", "a".repeat(CAT_GUID_HEX_RUN - 1));
        assert!(catalogue_display_text(&edge).is_some(), "31 hex digits is not a GUID");
        let over = format!("Hat {}", "a".repeat(CAT_GUID_HEX_RUN));
        assert_eq!(catalogue_display_text(&over), None, "32 hex digits is a bare GUID");
    }
}

rl_test! {
    fn display_refuses_control_and_bidi_characters() {
        for c in ['\u{0007}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}'] {
            let raw = format!("Hat{c}Name");
            assert_eq!(catalogue_display_text(&raw), None, "U+{:04X} must be refused", c as u32);
        }
        // Unicode whitespace (tab, U+2028/9) is collapsed, so it never reaches the payload.
        let sep = catalogue_display_text("Hat\u{2029}of\tTests");
        assert_eq!(sep.as_deref(), Some("Hat of Tests"));
    }
}

/// Every shape `bg3HoldsGuid` (contract `companion-dev-bridge-bg3-redaction.ts`)
/// refuses, each beside a near-miss the filter must keep.
fn refused_and_kept(refused: &str, kept: &str) {
    assert_eq!(
        catalogue_display_text(refused),
        None,
        "{refused:?} holds a GUID shape"
    );
    assert_eq!(
        catalogue_display_text(kept).as_deref(),
        Some(kept),
        "{kept:?} is a near-miss"
    );
}

rl_test! {
    fn display_refuses_a_guid_split_by_underscores() {
        refused_and_kept(
            "Hat 0a1b2c3d_0000_4e5f_8a9b_0c1d2e3f4a5b",
            "Hat 0a1b2c3d_0000_4e5f_8a9b_0c1d2e3f4a5",
        );
    }
}

rl_test! {
    fn display_refuses_a_guid_split_by_dots() {
        refused_and_kept(
            "Hat 0a1b2c3d.0000.4e5f.8a9b.0c1d2e3f4a5b",
            "Hat 0a1b2c3d.0000.4e5f.8a9b.0c1d2e3f4a5",
        );
    }
}

rl_test! {
    fn display_refuses_a_guid_with_no_separator_and_mixed_separators() {
        refused_and_kept("Hat 0a1b2c3d00004e5f8a9b0c1d2e3f4a5b", "Hat 0a1b2c3d00004e5f8a9b0c1d2e3f4a5");
        refused_and_kept(
            "Hat 0a1b2c3d-0000_4e5f.8a9b0c1d2e3f4a5b",
            "Hat 0a1b2c3d-0000_4e5f.8a9b0c1d2e3f4a5",
        );
    }
}

rl_test! {
    fn display_refuses_a_larian_handle_in_any_case() {
        refused_and_kept("Hat h6f2b8c1eg4d3a", "Hat h6f2b8c1eg4d3");
        refused_and_kept("Hat H6F2B8C1EG4D3A", "Hat h6f2b8c1xg4d3a");
    }
}

rl_test! {
    fn display_refuses_32_hex_once_separators_and_braces_are_stripped() {
        refused_and_kept(
            "Hat {0a1b2c3d__0000--4e5f..8a9b}0c1d2e3f4a5b",
            "Hat {0a1b2c3d__0000--4e5f..8a9b}0c1d2e3f4a5",
        );
        let spaced = "Hat 0a1b2c3d 0000 4e5f 8a9b 0c1d2e3f4a5b";
        assert_eq!(catalogue_display_text(spaced).as_deref(), Some(spaced), "a space is not a GUID separator");
    }
}

rl_test! {
    fn display_strips_invisible_format_chars_before_the_guid_and_digit_checks() {
        for c in ['\u{200B}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}', '\u{061C}', '\u{FEFF}', '\u{2060}'] {
            let digits = format!("Ring 12345{c}67890");
            assert_eq!(catalogue_display_text(&digits), None, "U+{:04X} hides a 10-digit run", c as u32);
            let guid = format!("Hat 0a1b2c3d{c}-0000-4e5f-8a9b-0c1d2e3f4a5b");
            assert_eq!(catalogue_display_text(&guid), None, "U+{:04X} hides a GUID", c as u32);
        }
        assert!(catalogue_display_text("Ring 1234\u{200B}5678").is_some(), "8 digits stay under the cap");
    }
}
