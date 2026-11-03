//! Browser build of the BG3 save reader (ROK-1960).
//!
//! One export, `inspectBg3Save(bytes)`, wraps [`rl_collectors`]: it reads the
//! `.lsv` package and returns the raw `SaveInfo.json` text plus the WebP
//! thumbnail and its blob ref, plus `gameIdKey`: the SHA-256 of `meta.lsf`'s
//! GameID, hashed here so the raw GameID never reaches JS (ROK-2070), or
//! `null` when it cannot be read. Normalising into `bg3-save/v1` stays in the
//! web app (the contract's TypeScript reference normaliser). A failure
//! throws an `Error` named `Bg3SaveError` whose `code` is one of
//! [`ErrorCode::as_str`].
//!
//! A WebAssembly trap is NOT a `Bg3SaveError`. A panic (logged to
//! `console.error` first by the hook [`inspect_bg3_save`] installs) or an
//! out-of-memory abort surfaces as a `WebAssembly.RuntimeError`
//! (`unreachable`, or a failed `memory.grow`). After a trap the instance's
//! state is undefined: callers must drop it and instantiate a fresh module
//! before the next call, and treat the upload as failed.

use js_sys::{Error, Object, Reflect, Uint8Array};
use rl_collectors::{
    CampaignKeySource, ExtractError, LspkError, Package, ThumbnailRef, campaign_key_from_package,
    extract_save_info, extract_thumbnail_webp, read_lspk, thumbnail_ref,
};
use wasm_bindgen::prelude::*;

#[cfg(test)]
mod tests;

/// Layout constants the shared test writer reads from `crate::lspk`.
#[cfg(test)]
mod lspk {
    pub(crate) use rl_collectors::LSPK_VERSION;
    pub(crate) const HEADER_LEN: usize = 40;
    pub(crate) const ENTRY_LEN: usize = 272;
}

#[wasm_bindgen(typescript_custom_section)]
const TS_TYPES: &str = r#"
export type Bg3SaveErrorCode =
  | "not_lspk"
  | "unsupported_version"
  | "unsupported_package"
  | "corrupt_package"
  | "missing_save_info"
  | "invalid_save_info";
export interface Bg3SaveThumbnail {
  sha256: string;
  byteLength: number;
  width: number;
  height: number;
}
export interface Bg3SaveInspection {
  saveInfo: string;
  webp: Uint8Array | null;
  thumbnail: Bg3SaveThumbnail | null;
  gameIdKey: string | null;
}
"#;

/// Why a package could not be inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Not an LSPK package: bad magic, or shorter than the 40-byte header.
    NotLspk,
    /// LSPK, but not version 18.
    UnsupportedVersion,
    /// Multi-part, too many files, or an unsupported compression method.
    UnsupportedPackage,
    /// A valid header, but the package is cut short after it, or the file
    /// table or an entry does not decompress (including a zstd frame that
    /// declares an oversized window).
    CorruptPackage,
    /// No root `SaveInfo.json`.
    MissingSaveInfo,
    /// `SaveInfo.json` is not JSON.
    InvalidSaveInfo,
}

impl ErrorCode {
    /// The stable code the web app matches on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotLspk => "not_lspk",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnsupportedPackage => "unsupported_package",
            Self::CorruptPackage => "corrupt_package",
            Self::MissingSaveInfo => "missing_save_info",
            Self::InvalidSaveInfo => "invalid_save_info",
        }
    }
}

/// What `inspectBg3Save` returns, before conversion to JS.
#[derive(Debug)]
pub struct Inspection {
    /// `SaveInfo.json` as compact JSON text (BOM stripped).
    pub save_info: String,
    /// The thumbnail bytes and ref; `None` when absent or unusable.
    pub thumbnail: Option<(Vec<u8>, ThumbnailRef)>,
    /// `sha256_hex(GameID)` (64 lowercase hex) when `meta.lsf` yields one;
    /// `None` otherwise. Never an error: the upload falls back to its
    /// folder or file-name key.
    pub game_id_key: Option<String>,
}

/// Read a `.lsv`. A missing or broken thumbnail is not an error.
pub fn inspect(bytes: &[u8]) -> Result<Inspection, (ErrorCode, String)> {
    let package = read_lspk(bytes).map_err(|e| (lspk_code(&e), e.to_string()))?;
    let info = extract_save_info(&package).map_err(|e| (extract_code(&e), e.to_string()))?;
    let thumbnail = extract_thumbnail_webp(&package)
        .ok()
        .and_then(|webp| thumbnail_ref(&webp).ok().map(|r| (webp, r)));
    Ok(Inspection {
        save_info: info.to_string(),
        thumbnail,
        game_id_key: game_id_key(&package),
    })
}

/// The `game-id` key, through the same rule the Companion uses (D2/D3).
fn game_id_key(package: &Package) -> Option<String> {
    match campaign_key_from_package(package, "") {
        (key, CampaignKeySource::GameId) => Some(key),
        _ => None,
    }
}

fn lspk_code(err: &LspkError) -> ErrorCode {
    match err {
        LspkError::BadMagic(_) | LspkError::Truncated { what: "header", .. } => ErrorCode::NotLspk,
        LspkError::UnsupportedVersion(_) => ErrorCode::UnsupportedVersion,
        LspkError::UnsupportedMultiPart(_)
        | LspkError::TooManyFiles(_)
        | LspkError::UnsupportedArchivePart { .. }
        | LspkError::UnsupportedCompression { .. } => ErrorCode::UnsupportedPackage,
        _ => ErrorCode::CorruptPackage,
    }
}

fn extract_code(err: &ExtractError) -> ErrorCode {
    match err {
        ExtractError::Lspk(inner) => lspk_code(inner),
        ExtractError::MissingSaveInfo => ErrorCode::MissingSaveInfo,
        _ => ErrorCode::InvalidSaveInfo,
    }
}

/// `inspectBg3Save(bytes: Uint8Array): Bg3SaveInspection`; throws a
/// `Bg3SaveError` with a `code` on failure.
#[wasm_bindgen(js_name = inspectBg3Save, unchecked_return_type = "Bg3SaveInspection")]
pub fn inspect_bg3_save(bytes: &[u8]) -> Result<JsValue, JsValue> {
    install_panic_hook();
    let inspection = inspect(bytes).map_err(|(code, message)| js_error(code, &message))?;
    let (webp, thumbnail) = match &inspection.thumbnail {
        Some((data, r)) => (
            JsValue::from(Uint8Array::from(data.as_slice())),
            thumbnail_js(r)?,
        ),
        None => (JsValue::NULL, JsValue::NULL),
    };
    let out = Object::new();
    set(&out, "saveInfo", &JsValue::from_str(&inspection.save_info))?;
    set(&out, "webp", &webp)?;
    set(&out, "thumbnail", &thumbnail)?;
    let game_id_key = inspection
        .game_id_key
        .as_deref()
        .map_or(JsValue::NULL, JsValue::from_str);
    set(&out, "gameIdKey", &game_id_key)?;
    Ok(out.into())
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(message: &str);
}

/// Log a panic's message to `console.error` before the module traps, so the
/// opaque `RuntimeError: unreachable` has a cause next to it. No-op natively.
fn install_panic_hook() {
    #[cfg(target_arch = "wasm32")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::panic::set_hook(Box::new(|info| {
                console_error(&format!("rl-collectors-web panicked: {info}"));
            }));
        });
    }
}

fn thumbnail_js(r: &ThumbnailRef) -> Result<JsValue, JsValue> {
    let out = Object::new();
    set(&out, "sha256", &JsValue::from_str(&r.sha256))?;
    set(&out, "byteLength", &JsValue::from_f64(r.byte_length as f64))?;
    set(&out, "width", &JsValue::from_f64(f64::from(r.width)))?;
    set(&out, "height", &JsValue::from_f64(f64::from(r.height)))?;
    Ok(out.into())
}

fn set(target: &Object, key: &str, value: &JsValue) -> Result<(), JsValue> {
    Reflect::set(target, &JsValue::from_str(key), value).map(drop)
}

fn js_error(code: ErrorCode, message: &str) -> JsValue {
    let err = Error::new(message);
    err.set_name("Bg3SaveError");
    // Setting a property on a fresh Error cannot fail.
    let _ = Reflect::set(
        &err,
        &JsValue::from_str("code"),
        &JsValue::from_str(code.as_str()),
    );
    err.into()
}
