use super::fixtures::{PLACEHOLDER_WEBP, act2, json, meta_from_golden, synthetic_save};
use super::writer::{FileSpec, Method, write_lspk};
use crate::{
    ExtractError, LspkError, WEBP_MAX_BYTES, WebpError, campaign_key_from_save_folder,
    extract_save_info, extract_thumbnail_webp, read_lspk, sha256_hex, thumbnail_ref,
    webp_dimensions,
};

/// RIFF/WEBP container whose first chunk is `fourcc` followed by `body`.
fn webp_with(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

rl_test! {
    fn extracts_save_info_and_strips_the_bom() {
        let bytes = synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP));
        let package = read_lspk(&bytes).expect("valid package");
        let value = extract_save_info(&package).expect("SaveInfo.json");
        assert_eq!(value, json(act2().saveinfo));
    }
}

rl_test! {
    fn extracts_save_info_without_a_bom() {
        let info = act2().saveinfo.as_bytes();
        let bytes = write_lspk(&[FileSpec { name: "SaveInfo.json", data: info, method: Method::Lz4 }]);
        let package = read_lspk(&bytes).expect("valid package");
        assert_eq!(extract_save_info(&package).expect("SaveInfo.json"), json(act2().saveinfo));
    }
}

rl_test! {
    fn reports_a_missing_or_broken_save_info() {
        let bytes = write_lspk(&[FileSpec { name: "meta.lsf", data: b"LSOF", method: Method::Stored }]);
        let package = read_lspk(&bytes).expect("valid package");
        let err = extract_save_info(&package).unwrap_err();
        assert!(matches!(err, ExtractError::MissingSaveInfo), "{err:?}");

        let bytes = write_lspk(&[FileSpec { name: "SaveInfo.json", data: b"{ not json", method: Method::ZstdRaw }]);
        let package = read_lspk(&bytes).expect("valid package");
        let err = extract_save_info(&package).unwrap_err();
        assert!(matches!(err, ExtractError::SaveInfoJson(_)), "{err:?}");
    }
}

rl_test! {
    fn extracts_the_root_webp_thumbnail() {
        let bytes = synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP));
        let package = read_lspk(&bytes).expect("valid package");
        assert_eq!(extract_thumbnail_webp(&package).expect("thumbnail"), PLACEHOLDER_WEBP);
    }
}

rl_test! {
    fn reports_a_missing_thumbnail_and_ignores_nested_webps() {
        let bytes = synthetic_save(act2().saveinfo, None);
        let package = read_lspk(&bytes).expect("valid package");
        let err = extract_thumbnail_webp(&package).unwrap_err();
        assert!(matches!(err, ExtractError::MissingThumbnail), "{err:?}");

        let bytes = write_lspk(&[FileSpec { name: "Previews/x.webp", data: PLACEHOLDER_WEBP, method: Method::Stored }]);
        let package = read_lspk(&bytes).expect("valid package");
        let err = extract_thumbnail_webp(&package).unwrap_err();
        assert!(matches!(err, ExtractError::MissingThumbnail), "{err:?}");
    }
}

rl_test! {
    fn rejects_a_thumbnail_that_is_not_webp_or_too_large() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01";
        let bytes = write_lspk(&[FileSpec { name: "QuickSave_1.WebP", data: png, method: Method::Stored }]);
        let package = read_lspk(&bytes).expect("valid package");
        let err = extract_thumbnail_webp(&package).unwrap_err();
        assert!(matches!(err, ExtractError::InvalidWebp(WebpError::NotWebp)), "{err:?}");

        let big = vec![0_u8; WEBP_MAX_BYTES as usize + 1];
        let bytes = write_lspk(&[FileSpec { name: "Big.WebP", data: &big, method: Method::Stored }]);
        let package = read_lspk(&bytes).expect("valid package");
        let err = extract_thumbnail_webp(&package).unwrap_err();
        assert!(matches!(err, ExtractError::Lspk(LspkError::EntryTooLarge { .. })), "{err:?}");
    }
}

rl_test! {
    fn thumbnail_ref_matches_the_golden_fixture() {
        let golden = json(act2().golden);
        let expected = meta_from_golden(&golden).thumbnail.expect("act2 has a thumbnail");
        assert_eq!(thumbnail_ref(PLACEHOLDER_WEBP).expect("ref"), expected);
        assert_eq!(expected.width, 1280);
        assert_eq!(expected.height, 720);
    }
}

rl_test! {
    fn thumbnail_ref_refuses_oversize_blobs() {
        let big = vec![0_u8; WEBP_MAX_BYTES as usize + 1];
        assert_eq!(
            thumbnail_ref(&big),
            Err(WebpError::TooLarge { size: WEBP_MAX_BYTES + 1, limit: WEBP_MAX_BYTES })
        );
    }
}

rl_test! {
    fn reads_lossless_and_extended_webp_dimensions() {
        let bits: u32 = (100 - 1) | ((50 - 1) << 14);
        let mut body = vec![0x2f];
        body.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(webp_dimensions(&webp_with(b"VP8L", &body)), Ok((100, 50)));

        let mut body = vec![0_u8; 4];
        body.extend_from_slice(&(640_u32 - 1).to_le_bytes()[..3]);
        body.extend_from_slice(&(480_u32 - 1).to_le_bytes()[..3]);
        assert_eq!(webp_dimensions(&webp_with(b"VP8X", &body)), Ok((640, 480)));
    }
}

rl_test! {
    fn rejects_malformed_webp_headers() {
        assert_eq!(webp_dimensions(b"RIFF"), Err(WebpError::NotWebp));
        assert_eq!(webp_dimensions(&PLACEHOLDER_WEBP[..25]), Err(WebpError::Truncated));
        assert_eq!(webp_dimensions(&webp_with(b"VP8L", &[0x00, 1, 2, 3, 4])), Err(WebpError::BadVp8l));
        assert_eq!(
            webp_dimensions(&webp_with(b"ALPH", &[0; 10])),
            Err(WebpError::UnsupportedChunk("ALPH".to_owned()))
        );
        let mut bad_start = PLACEHOLDER_WEBP.to_vec();
        bad_start[23] = 0;
        assert_eq!(webp_dimensions(&bad_start), Err(WebpError::BadVp8));
        let mut zero = PLACEHOLDER_WEBP.to_vec();
        zero[26] = 0;
        zero[27] = 0;
        assert_eq!(webp_dimensions(&zero), Err(WebpError::ZeroDimension { width: 0, height: 720 }));
    }
}

rl_test! {
    fn hashes_campaign_keys_as_lowercase_sha256() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(campaign_key_from_save_folder("abc"), sha256_hex(b"abc"));
        assert_eq!(campaign_key_from_save_folder("Tav-1234").len(), 64);
    }
}
