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

/// A save with `SaveInfo.json` (LZ4) and, optionally, a root thumbnail.
fn save(info: &[u8], webp: Option<&[u8]>) -> Vec<u8> {
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
        let err = crate::inspect_bg3_save(b"nope").expect_err("throws");
        assert_eq!(get(&err, "code").as_string().as_deref(), Some("not_lspk"));
        assert_eq!(get(&err, "name").as_string().as_deref(), Some("Bg3SaveError"));
    }
}
