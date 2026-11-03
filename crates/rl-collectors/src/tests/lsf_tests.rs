//! LSF v7 reader and GameID campaign key (ROK-2070). Synthetic resources
//! from `lsf_writer` only; the real-save check is L2's.

use super::fixtures::{PLACEHOLDER_WEBP, act2, synthetic_save};
use super::lsf_writer::{
    Attr, Codec, INT32, LZ4, STRING_GAME_ID, Sections, UUID_BYTES, UUID_TEXT, build_sections, lsf,
    meta_attrs, string_attr, uuid_attr, write_lsf,
};
use super::writer::{FileSpec, Method, set_u32, write_lspk};
use crate::{
    CampaignKeySource, LSF_MAX_SECTION_BYTES, LsfError, META_LSF_MAX_BYTES,
    campaign_key_from_package, campaign_key_from_save_folder, game_id_from_package,
    read_lsf_attribute, read_lspk, sha256_hex,
};

const FOLDER: &str = "QuickSave_42";

fn codecs() -> Vec<Codec> {
    vec![
        Codec::Stored,
        LZ4,
        // Block + content checksums and a content size, tiny blocks.
        Codec::Lz4 {
            flg: 0x7C,
            chunk: 7,
        },
        // Linked blocks: each block may reference the previous output.
        Codec::Lz4 {
            flg: 0x40,
            chunk: 16,
        },
        Codec::Zstd,
    ]
}

fn game_id(bytes: &[u8]) -> Result<Option<String>, LsfError> {
    read_lsf_attribute(bytes, "GameID")
}

fn string_meta(codec: Codec, v3: bool) -> Vec<u8> {
    lsf(
        &meta_attrs(string_attr("GameID", STRING_GAME_ID)),
        codec,
        v3,
    )
}

/// A save package whose `meta.lsf` is `meta`, LZ4-packed like the real one.
fn save_with_meta(meta: &[u8]) -> Vec<u8> {
    write_lspk(&[FileSpec {
        name: "meta.lsf",
        data: meta,
        method: Method::Lz4,
    }])
}

fn key_of(package_bytes: &[u8]) -> (String, CampaignKeySource) {
    let package = read_lspk(package_bytes).expect("package parses");
    campaign_key_from_package(&package, FOLDER)
}

fn fallback() -> (String, CampaignKeySource) {
    (
        campaign_key_from_save_folder(FOLDER),
        CampaignKeySource::SaveFolder,
    )
}

rl_test! {
    fn reads_a_string_and_a_uuid_game_id_through_every_codec_and_record_shape() {
        for codec in codecs() {
            for v3 in [true, false] {
                let case = format!("{codec:?} v3={v3}");
                let text = game_id(&string_meta(codec, v3));
                assert_eq!(text, Ok(Some(STRING_GAME_ID.to_owned())), "string GameID, {case}");
                let uuid = lsf(&meta_attrs(uuid_attr("GameID", UUID_BYTES)), codec, v3);
                assert_eq!(game_id(&uuid), Ok(Some(UUID_TEXT.to_owned())), "UUID GameID, {case}");
            }
        }
    }
}

rl_test! {
    fn reads_every_other_attribute_by_name_too() {
        let bytes = string_meta(LZ4, true);
        let save_time = read_lsf_attribute(&bytes, "SaveTime");
        assert_eq!(save_time, Ok(Some("2026-11-02T00:00:00".to_owned())));
        let level = read_lsf_attribute(&bytes, "LevelUniqueKey");
        assert_eq!(level, Ok(Some("WLD_Main_A_synthetic".to_owned())));
    }
}

rl_test! {
    fn accepts_every_string_type_with_or_without_a_nul() {
        for type_id in [20, 21, 22, 23, 29, 30] {
            for value in [b"abc\0".to_vec(), b"abc".to_vec()] {
                let attr = Attr { name: "GameID", type_id, value };
                let bytes = lsf(&meta_attrs(attr), Codec::Stored, true);
                assert_eq!(game_id(&bytes), Ok(Some("abc".to_owned())), "type {type_id}");
            }
        }
    }
}

rl_test! {
    fn an_absent_attribute_is_none() {
        let bytes = lsf(&[string_attr("SaveTime", "x")], LZ4, true);
        assert_eq!(game_id(&bytes), Ok(None));
    }
}

rl_test! {
    fn a_keys_section_after_the_values_is_ignored() {
        let mut s = build_sections(&meta_attrs(string_attr("GameID", "k")), true);
        s.keys = vec![0xEE; 40];
        for codec in codecs() {
            let bytes = write_lsf(&s, codec, true);
            assert_eq!(game_id(&bytes), Ok(Some("k".to_owned())), "{codec:?}");
        }
    }
}

rl_test! {
    fn refuses_a_bad_magic_and_any_version_but_7() {
        let mut bytes = string_meta(Codec::Stored, true);
        bytes[..4].copy_from_slice(b"LSOX");
        assert_eq!(game_id(&bytes), Err(LsfError::BadMagic(*b"LSOX")));
        let placeholder = b"LSOF synthetic meta placeholder LSOF synthetic meta placeholder";
        let version = u32::from_le_bytes(*b" syn");
        assert_eq!(game_id(placeholder), Err(LsfError::UnsupportedVersion(version)));
        for v in [6, 8] {
            let mut bytes = string_meta(Codec::Stored, true);
            set_u32(&mut bytes, 4, v);
            assert_eq!(game_id(&bytes), Err(LsfError::UnsupportedVersion(v)));
        }
    }
}

rl_test! {
    fn refuses_zlib_and_unknown_compression() {
        for (flags, method) in [(0x21, 1), (0x2F, 15), (0x04, 4)] {
            let bytes = string_meta(Codec::Raw(flags), true);
            assert_eq!(game_id(&bytes), Err(LsfError::UnsupportedCompression(method)));
        }
    }
}

rl_test! {
    fn refuses_a_section_over_the_cap_before_reading_it() {
        let mut bytes = string_meta(Codec::Stored, true);
        let size = LSF_MAX_SECTION_BYTES + 1;
        set_u32(&mut bytes, 16, size as u32);
        let expected = LsfError::SectionTooLarge {
            section: "strings",
            size,
            limit: LSF_MAX_SECTION_BYTES,
        };
        assert_eq!(game_id(&bytes), Err(expected));
    }
}

rl_test! {
    fn reports_the_short_section_on_truncation() {
        assert!(matches!(game_id(b""), Err(LsfError::Truncated { what: "header", .. })));
        let full = string_meta(Codec::Stored, true);
        let err = game_id(&full[..full.len() - 1]);
        assert!(matches!(err, Err(LsfError::Truncated { what: "values", .. })), "{err:?}");
    }
}

/// Run `edit` on fresh stored V3 sections and return the read result.
fn with_sections(edit: impl FnOnce(&mut Sections)) -> Result<Option<String>, LsfError> {
    let mut s = build_sections(&meta_attrs(string_attr("GameID", "g")), true);
    edit(&mut s);
    game_id(&write_lsf(&s, Codec::Stored, true))
}

fn decode_error_in(result: Result<Option<String>, LsfError>, wanted: &str) {
    match result {
        Err(LsfError::Decode { section, .. }) => assert_eq!(section, wanted),
        other => panic!("expected a {wanted} decode error, got {other:?}"),
    }
}

rl_test! {
    fn reports_malformed_sections_as_decode_errors() {
        decode_error_in(with_sections(|s| s.attrs.push(0)), "attributes");
        decode_error_in(with_sections(|s| s.strings.truncate(9)), "strings");
        decode_error_in(with_sections(|s| s.strings[10] = 0xFF), "strings");
        let too_many = with_sections(|s| set_u32(&mut s.strings, 0, 70_000));
        let expected = LsfError::Decode {
            section: "strings",
            reason: "too many string buckets".to_owned(),
        };
        assert_eq!(too_many, Err(expected), "the bucket cap refuses before reading");
        // The GameID record is the second one; point its offset past the end.
        decode_error_in(with_sections(|s| set_u32(&mut s.attrs, 16 + 12, 9_999)), "values");
        // Plain bytes flagged LZ4 are not a valid LZ4 block.
        decode_error_in(game_id(&string_meta(Codec::Raw(0x22), true)), "strings");
        // Frame version bits 00, and a dictionary id, are refused.
        for flg in [0x20, 0x61] {
            let frame = lsf(&meta_attrs(string_attr("GameID", "g")), Codec::Lz4 { flg, chunk: 64 }, true);
            decode_error_in(game_id(&frame), "attributes");
        }
    }
}

rl_test! {
    fn refuses_a_game_id_that_is_neither_string_nor_uuid() {
        let int = Attr { name: "GameID", type_id: INT32, value: vec![1, 0, 0, 0] };
        decode_error_in(game_id_result(int), "values");
        let short_uuid = Attr { name: "GameID", type_id: 31, value: vec![0; 15] };
        decode_error_in(game_id_result(short_uuid), "values");
        let latin1 = Attr { name: "GameID", type_id: 22, value: vec![0xE9, 0] };
        decode_error_in(game_id_result(latin1), "values");
    }
}

fn game_id_result(attr: Attr) -> Result<Option<String>, LsfError> {
    game_id(&lsf(&meta_attrs(attr), Codec::Stored, false))
}

rl_test! {
    fn reports_a_name_ref_outside_the_string_table() {
        let bad = (7u32 << 16) | 1;
        let result = with_sections(|s| set_u32(&mut s.attrs, 0, bad));
        assert_eq!(result, Err(LsfError::BadStringRef(bad)));
    }
}

rl_test! {
    fn every_truncation_fails_cleanly_for_every_codec() {
        for codec in codecs() {
            for v3 in [true, false] {
                let full = string_meta(codec, v3);
                for len in 0..full.len() {
                    assert!(game_id(&full[..len]).is_err(), "{codec:?} v3={v3}: {len}-byte prefix read");
                }
            }
        }
    }
}

/// Deterministic 64-bit LCG (Knuth MMIX constants).
fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 16
}

rl_test! {
    fn random_corruption_never_panics() {
        let mut rng = 0x2070_u64;
        for codec in codecs() {
            let full = string_meta(codec, true);
            for _ in 0..1500 {
                let mut bytes = full.clone();
                let r = lcg(&mut rng);
                let at = (r as usize) % bytes.len();
                bytes[at] ^= ((r >> 24) as u8) | 1;
                let _ = game_id(&bytes);
            }
        }
    }
}

rl_test! {
    fn keys_a_readable_save_by_its_game_id() {
        let bytes = save_with_meta(&string_meta(LZ4, true));
        let expected = (sha256_hex(STRING_GAME_ID.as_bytes()), CampaignKeySource::GameId);
        assert_eq!(key_of(&bytes), expected);
        let uuid = save_with_meta(&lsf(&meta_attrs(uuid_attr("GameID", UUID_BYTES)), Codec::Zstd, false));
        assert_eq!(key_of(&uuid), (sha256_hex(UUID_TEXT.as_bytes()), CampaignKeySource::GameId));
        let package = read_lspk(&bytes).expect("package parses");
        assert_eq!(game_id_from_package(&package).as_deref(), Some(STRING_GAME_ID));
    }
}

rl_test! {
    fn the_placeholder_meta_lsf_falls_back_to_the_save_folder() {
        let bytes = synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP));
        assert_eq!(key_of(&bytes), fallback());
    }
}

rl_test! {
    fn a_missing_or_oversize_meta_lsf_falls_back() {
        let none = write_lspk(&[FileSpec { name: "SaveInfo.json", data: b"{}", method: Method::Stored }]);
        assert_eq!(key_of(&none), fallback());
        let mut attrs = meta_attrs(string_attr("GameID", STRING_GAME_ID));
        attrs.push(string_attr("Filler", &"f".repeat(META_LSF_MAX_BYTES as usize)));
        let oversize = lsf(&attrs, Codec::Stored, true);
        assert!(oversize.len() as u64 > META_LSF_MAX_BYTES);
        assert_eq!(game_id(&oversize), Ok(Some(STRING_GAME_ID.to_owned())), "readable, only too big");
        assert_eq!(key_of(&save_with_meta(&oversize)), fallback());
    }
}

rl_test! {
    fn a_broken_meta_lsf_falls_back() {
        let mut bad_magic = string_meta(LZ4, true);
        bad_magic[0] = b'X';
        let mut truncated = string_meta(LZ4, true);
        truncated.pop();
        let cases = [
            ("bad magic", bad_magic),
            ("zlib", string_meta(Codec::Raw(0x21), true)),
            ("truncated", truncated),
            ("no GameID", lsf(&[string_attr("SaveTime", "x")], LZ4, true)),
        ];
        for (case, meta) in cases {
            assert_eq!(key_of(&save_with_meta(&meta)), fallback(), "{case}");
        }
    }
}

rl_test! {
    fn a_game_id_outside_1_to_128_printable_ascii_falls_back() {
        let longest = "a".repeat(128);
        let accepted = save_with_meta(&lsf(&meta_attrs(string_attr("GameID", &longest)), LZ4, true));
        assert_eq!(key_of(&accepted).1, CampaignKeySource::GameId, "128 bytes is accepted");
        for bad in [String::new(), "a".repeat(129), "tab\there".to_owned(), "é".to_owned()] {
            let meta = lsf(&meta_attrs(string_attr("GameID", &bad)), LZ4, true);
            assert_eq!(key_of(&save_with_meta(&meta)), fallback(), "GameID {bad:?}");
        }
    }
}
