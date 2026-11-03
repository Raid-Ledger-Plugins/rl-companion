//! Runs natively (`cargo test`) and as WASM (`wasm-pack test --node`).
//! Packages are built by rl-collectors' test-only LSPK writer from the
//! contract's anonymised fixtures; no real save data.

/// Declare a test that runs under `cargo test` and under wasm-bindgen-test.
macro_rules! rl_test {
    ($(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[cfg_attr(not(target_arch = "wasm32"), test)]
        #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
        fn $name() $body
    };
}

#[allow(dead_code)]
#[path = "../../../rl-collectors/src/tests/writer.rs"]
mod writer;

use rl_collectors::sha256_hex;
use serde_json::Value;

use crate::{ErrorCode, inspect};
use writer::{FileSpec, Method, set_u32, write_lspk};

const SAVEINFO: &str = include_str!(
    "../../../../contracts/bg3-save/v1/fixtures/saveinfo/act2-party-4p-summon.saveinfo.json"
);

/// Synthetic flat-grey 1280×720 WebP from the contract.
const WEBP: &[u8] =
    include_bytes!("../../../../contracts/bg3-save/v1/fixtures/placeholder-thumbnail.webp");

/// The anonymised real-shape `meta.lsf` (ROK-2070 L2): its FixedString
/// GameID is [`SYNTHETIC_GAME_ID`]; every other string is zeroed or filler.
const META_LSF: &[u8] =
    include_bytes!("../../../rl-collectors/tests/fixtures/meta/real-shape-string.meta.lsf");
const SYNTHETIC_GAME_ID: &str = "00000000-2070-4000-8000-000000000001";

/// A save with `SaveInfo.json` (LZ4) and, optionally, a root thumbnail.
fn save(info: &[u8], webp: Option<&[u8]>) -> Vec<u8> {
    save_with_meta(info, webp, None)
}

/// [`save`] plus, optionally, a root `meta.lsf` (LZ4, as real saves pack it).
fn save_with_meta(info: &[u8], webp: Option<&[u8]>, meta: Option<&[u8]>) -> Vec<u8> {
    let mut files = vec![FileSpec {
        name: "SaveInfo.json",
        data: info,
        method: Method::Lz4,
    }];
    if let Some(data) = webp {
        files.push(FileSpec {
            name: "Synthetic.WebP",
            data,
            method: Method::Stored,
        });
    }
    if let Some(data) = meta {
        files.push(FileSpec {
            name: "meta.lsf",
            data,
            method: Method::Lz4,
        });
    }
    write_lspk(&files)
}

fn with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xEF, 0xBB, 0xBF];
    out.extend_from_slice(text.as_bytes());
    out
}

fn code(bytes: &[u8]) -> ErrorCode {
    inspect(bytes).expect_err("inspect must fail").0
}

rl_test! {
    fn returns_save_info_text_and_the_thumbnail() {
        let got = inspect(&save(&with_bom(SAVEINFO), Some(WEBP))).expect("valid save");
        let parsed: Value = serde_json::from_str(&got.save_info).expect("saveInfo is JSON text");
        let expected: Value = serde_json::from_str(SAVEINFO).expect("fixture is JSON");
        assert_eq!(parsed, expected);
        let (data, r) = got.thumbnail.expect("thumbnail present");
        assert_eq!(data, WEBP);
        assert_eq!(r.sha256, sha256_hex(WEBP));
        assert_eq!(r.byte_length, WEBP.len() as u64);
        assert_eq!((r.width, r.height), (1280, 720));
    }
}

rl_test! {
    fn a_missing_or_broken_thumbnail_is_null_not_an_error() {
        let none = inspect(&save(SAVEINFO.as_bytes(), None)).expect("no thumbnail");
        assert!(none.thumbnail.is_none(), "absent thumbnail must be None");
        let junk = inspect(&save(SAVEINFO.as_bytes(), Some(b"not a webp"))).expect("bad webp");
        assert!(junk.thumbnail.is_none(), "unreadable thumbnail must be None");
    }
}

rl_test! {
    fn game_id_key_is_the_hash_of_a_readable_game_id_else_null() {
        let plain = inspect(&save(SAVEINFO.as_bytes(), Some(WEBP))).expect("synthetic act2");
        assert_eq!(plain.game_id_key, None, "no meta.lsf → no gameIdKey");
        let broken = inspect(&save_with_meta(SAVEINFO.as_bytes(), None, Some(b"LSOF junk")))
            .expect("a broken meta.lsf never fails the inspection");
        assert_eq!(broken.game_id_key, None, "unreadable meta.lsf → no gameIdKey");
        let real = inspect(&save_with_meta(SAVEINFO.as_bytes(), Some(WEBP), Some(META_LSF)))
            .expect("save with the anonymised meta.lsf");
        let want = sha256_hex(SYNTHETIC_GAME_ID.as_bytes());
        assert_eq!(real.game_id_key.as_deref(), Some(want.as_str()), "gameIdKey = sha256(GameID)");
        assert_eq!(want.len(), 64, "64 lowercase hex");
    }
}

rl_test! {
    fn maps_reader_failures_to_stable_codes() {
        let valid = save(SAVEINFO.as_bytes(), None);
        assert_eq!(code(&[b'N', b'O', b'P', b'E', 0, 0, 0, 0]), ErrorCode::NotLspk);
        assert_eq!(code(&valid[..20]), ErrorCode::NotLspk);
        let mut v17 = valid;
        set_u32(&mut v17, 4, 17);
        assert_eq!(code(&v17), ErrorCode::UnsupportedVersion);
        let zlib = write_lspk(&[FileSpec { name: "SaveInfo.json", data: b"{}", method: Method::Zlib }]);
        assert_eq!(code(&zlib), ErrorCode::UnsupportedPackage);
        let none = write_lspk(&[FileSpec { name: "meta.lsf", data: b"LSOF", method: Method::Stored }]);
        assert_eq!(code(&none), ErrorCode::MissingSaveInfo);
        assert_eq!(code(&save(b"{not json", None)), ErrorCode::InvalidSaveInfo);
    }
}

rl_test! {
    fn damage_after_a_valid_header_is_corrupt_package() {
        let valid = save(SAVEINFO.as_bytes(), None);
        let cut_table = inspect(&valid[..valid.len() - 3]).expect_err("cut file table");
        assert_eq!(cut_table.0, ErrorCode::CorruptPackage, "{}", cut_table.1);
        let header_only = inspect(&valid[..44]).expect_err("header, no table");
        assert_eq!(header_only.0, ErrorCode::CorruptPackage, "{}", header_only.1);
        let bad_lz4 = write_lspk(&[FileSpec {
            name: "SaveInfo.json",
            data: SAVEINFO.as_bytes(),
            method: Method::Precompressed(0x22, b"\xff\xff\xff"),
        }]);
        let (c, msg) = inspect(&bad_lz4).expect_err("3 bytes cannot hold SaveInfo.json");
        assert_eq!(c, ErrorCode::CorruptPackage, "{msg}");
        assert!(msg.contains("cannot hold"), "{msg}");
    }
}

rl_test! {
    fn reads_zstd_save_info_and_refuses_an_oversized_window() {
        let zstd = write_lspk(&[FileSpec { name: "SaveInfo.json", data: SAVEINFO.as_bytes(), method: Method::ZstdRaw }]);
        let got = inspect(&zstd).expect("zstd SaveInfo.json");
        let parsed: Value = serde_json::from_str(&got.save_info).expect("saveInfo is JSON text");
        assert_eq!(parsed, serde_json::from_str::<Value>(SAVEINFO).expect("fixture is JSON"));
        // Same 2-byte raw-block frame, window byte 0x00 (1 KiB) vs 0x80 (64 MiB).
        const SMALL: &[u8] = &[0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x00, 0x11, 0x00, 0x00, b'{', b'}'];
        const BIG: &[u8] = &[0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x80, 0x11, 0x00, 0x00, b'{', b'}'];
        let frame = |bytes: &'static [u8]| {
            write_lspk(&[FileSpec { name: "SaveInfo.json", data: b"{}", method: Method::Precompressed(0x23, bytes) }])
        };
        assert_eq!(inspect(&frame(SMALL)).expect("1 KiB window").save_info, "{}");
        let (c, msg) = inspect(&frame(BIG)).expect_err("64 MiB window on a 2-byte entry");
        assert_eq!(c, ErrorCode::CorruptPackage, "{msg}");
        assert!(msg.contains("67108864-byte window"), "{msg}");
    }
}

/// CI writes the synthetic saves Raid Ledger vendors into `pkg/fixtures/`
/// (`RL_FIXTURE_OUT`): each `.lsv` plus what `inspectBg3Save` must return
/// for it (minus the `webp` bytes, which are the contract's placeholder).
/// `synthetic-act2` has no `meta.lsf` (`gameIdKey: null`);
/// `synthetic-act2-game-id` adds the anonymised one (ROK-2070).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn export_synthetic_fixture_for_raid_ledger() {
    let Some(dir) = std::env::var_os("RL_FIXTURE_OUT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let info = with_bom(SAVEINFO);
    let plain = save(&info, Some(WEBP));
    export_fixture(&dir, "synthetic-act2", &plain, false);
    let game_id = save_with_meta(&info, Some(WEBP), Some(META_LSF));
    export_fixture(&dir, "synthetic-act2-game-id", &game_id, true);
}

#[cfg(not(target_arch = "wasm32"))]
fn export_fixture(dir: &std::path::Path, name: &str, bytes: &[u8], has_game_id: bool) {
    let got = inspect(bytes).expect("fixture inspects");
    assert_eq!(
        got.game_id_key.is_some(),
        has_game_id,
        "{name}: gameIdKey presence"
    );
    let (_, r) = got.thumbnail.expect("fixture has a thumbnail");
    let expected = serde_json::json!({
        "saveInfo": got.save_info,
        "thumbnail": {
            "sha256": r.sha256,
            "byteLength": r.byte_length,
            "width": r.width,
            "height": r.height,
        },
        "gameIdKey": got.game_id_key,
    });
    let text = serde_json::to_string_pretty(&expected).expect("serialise expected");
    std::fs::write(dir.join(format!("{name}.lsv")), bytes).expect("write .lsv");
    std::fs::write(dir.join(format!("{name}.expected.json")), text + "\n").expect("write expected");
}

rl_test! {
    fn error_code_strings_match_the_web_contract() {
        let all = [
            (ErrorCode::NotLspk, "not_lspk"),
            (ErrorCode::UnsupportedVersion, "unsupported_version"),
            (ErrorCode::UnsupportedPackage, "unsupported_package"),
            (ErrorCode::CorruptPackage, "corrupt_package"),
            (ErrorCode::MissingSaveInfo, "missing_save_info"),
            (ErrorCode::InvalidSaveInfo, "invalid_save_info"),
        ];
        for (code, text) in all {
            assert_eq!(code.as_str(), text);
        }
    }
}

#[cfg(target_arch = "wasm32")]
rl_test! {
    fn js_export_returns_a_plain_object_and_throws_coded_errors() {
        use js_sys::{Reflect, Uint8Array};
        use wasm_bindgen::JsValue;
        let out = crate::inspect_bg3_save(&save(SAVEINFO.as_bytes(), Some(WEBP))).expect("ok");
        let get = |v: &JsValue, k: &str| Reflect::get(v, &JsValue::from_str(k)).expect("get");
        assert!(get(&out, "saveInfo").is_string(), "saveInfo must be a string");
        assert_eq!(Uint8Array::new(&get(&out, "webp")).to_vec(), WEBP);
        let thumb = get(&out, "thumbnail");
        assert_eq!(get(&thumb, "sha256").as_string(), Some(sha256_hex(WEBP)));
        assert_eq!(get(&thumb, "byteLength").as_f64(), Some(WEBP.len() as f64));
        assert_eq!(get(&thumb, "width").as_f64(), Some(1280.0));
        let bare = crate::inspect_bg3_save(&save(SAVEINFO.as_bytes(), None)).expect("ok");
        assert!(get(&bare, "webp").is_null(), "webp must be null without a thumbnail");
        assert!(get(&bare, "thumbnail").is_null(), "thumbnail must be null");
        assert!(get(&bare, "gameIdKey").is_null(), "gameIdKey must be null without meta.lsf");
        let keyed = crate::inspect_bg3_save(&save_with_meta(SAVEINFO.as_bytes(), None, Some(META_LSF)))
            .expect("ok");
        assert_eq!(
            get(&keyed, "gameIdKey").as_string(),
            Some(sha256_hex(SYNTHETIC_GAME_ID.as_bytes())),
            "gameIdKey must be sha256(GameID)"
        );
        let err = crate::inspect_bg3_save(b"nope").expect_err("throws");
        assert_eq!(get(&err, "code").as_string().as_deref(), Some("not_lspk"));
        assert_eq!(get(&err, "name").as_string().as_deref(), Some("Bg3SaveError"));
    }
}
