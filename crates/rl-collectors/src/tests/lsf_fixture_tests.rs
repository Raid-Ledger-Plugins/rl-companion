//! The anonymised real-shape `meta.lsf` (ROK-2070 L2) and the env-gated
//! real-save check. See `tests/fixtures/meta/README.md` for the transform.

use super::writer::{FileSpec, Method, write_lspk};
use crate::{
    CampaignKeySource, campaign_key_from_package, read_lsf_attribute, read_lspk, sha256_hex,
};

const REAL_SHAPE: &[u8] = include_bytes!("../../tests/fixtures/meta/real-shape-string.meta.lsf");
const SYNTHETIC_GAME_ID: &str = "00000000-2070-4000-8000-000000000001";

fn attr(name: &str) -> Option<String> {
    match read_lsf_attribute(REAL_SHAPE, name) {
        Ok(value) => value,
        Err(e) => panic!("the real-shape fixture must parse, reading {name} failed: {e}"),
    }
}

rl_test! {
    fn the_real_shape_fixture_has_the_real_layout() {
        let u32_at = |at: usize| u32::from_le_bytes([
            REAL_SHAPE[at], REAL_SHAPE[at + 1], REAL_SHAPE[at + 2], REAL_SHAPE[at + 3],
        ]);
        assert_eq!(&REAL_SHAPE[..4], b"LSOF");
        assert_eq!(u32_at(4), 7, "version");
        assert_eq!(REAL_SHAPE[56], 0x22, "CompressionFlags: LZ4");
        assert_eq!(u32_at(60), 0, "MetadataFormat 0: V2 records");
        assert_eq!((u32_at(24), u32_at(28)), (0, 0), "Keys section is empty");
    }
}

rl_test! {
    fn the_real_shape_fixture_yields_the_synthetic_game_id() {
        assert_eq!(attr("GameID").as_deref(), Some(SYNTHETIC_GAME_ID));
    }
}

rl_test! {
    fn the_real_shape_fixture_keys_by_the_synthetic_game_id() {
        let save = write_lspk(&[FileSpec {
            name: "meta.lsf",
            data: REAL_SHAPE,
            method: Method::Stored,
        }]);
        let package = read_lspk(&save).expect("synthetic package");
        assert_eq!(
            campaign_key_from_package(&package, "QuickSave_42"),
            (
                sha256_hex(SYNTHETIC_GAME_ID.as_bytes()),
                CampaignKeySource::GameId
            )
        );
    }
}

rl_test! {
    fn the_real_shape_fixture_carries_no_other_string_data() {
        for name in ["GameSessionID", "LeaderName", "MD5", "Name", "Folder", "LevelUniqueKey"] {
            let value = attr(name).unwrap_or_else(|| panic!("{name} is missing"));
            assert!(
                !value.is_empty() && value.bytes().all(|b| b == b'x'),
                "{name} should be all 'x' filler, got {} bytes with other content",
                value.len()
            );
        }
    }
}

/// `RL_REAL_SAVE=<save.lsv> cargo test -p rl-collectors -- --ignored real_save`
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "needs a real BG3 save: RL_REAL_SAVE=<path to .lsv>"]
fn a_real_save_is_keyed_by_its_game_id() {
    let path = std::env::var("RL_REAL_SAVE").expect("set RL_REAL_SAVE to a real .lsv");
    let bytes = std::fs::read(&path).expect("read RL_REAL_SAVE");
    let package = read_lspk(&bytes).expect("a real save is LSPK v18");
    let id = crate::game_id_from_package(&package);
    let Some(id) = id else {
        panic!("a real save must yield Some(GameID), got None");
    };
    let (key, source) = campaign_key_from_package(&package, "folder");
    assert_eq!(source, CampaignKeySource::GameId, "key source");
    assert_eq!(
        key,
        sha256_hex(id.as_bytes()),
        "key is SHA-256 of the GameID"
    );
}
