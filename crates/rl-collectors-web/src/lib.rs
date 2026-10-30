//! Browser build of the BG3 save reader (ROK-1960).
//!
//! One export, `inspectBg3Save(bytes)`, wraps [`rl_collectors`]: it reads the
//! `.lsv` package and returns the raw `SaveInfo.json` text plus the WebP
//! thumbnail and its blob ref. Normalising into `bg3-save/v1` stays in the
//! web app (the contract's TypeScript reference normaliser). A failure
//! throws an `Error` named `Bg3SaveError` whose `code` is one of
//! [`ErrorCode::as_str`].

use js_sys::{Error, Object, Reflect, Uint8Array};
use rl_collectors::{
    ExtractError, LspkError, ThumbnailRef, extract_save_info, extract_thumbnail_webp, read_lspk,
    thumbnail_ref,
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
}
"#;

/// Why a package could not be inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Not an LSPK package, or cut short.
    NotLspk,
    /// LSPK, but not version 18.
    UnsupportedVersion,
    /// Multi-part, too many files, or an unsupported compression method.
    UnsupportedPackage,
    /// The file table or an entry does not decompress.
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
    })
}

fn lspk_code(err: &LspkError) -> ErrorCode {
    match err {
        LspkError::BadMagic(_) | LspkError::Truncated { .. } => ErrorCode::NotLspk,
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
    let inspection = inspect(bytes).map_err(|(code, message)| js_error(code, &message))?;
    let (webp, thumbnail) = match &inspection.thumbnail {
        Some((data, r)) => (JsValue::from(Uint8Array::from(data.as_slice())), thumbnail_js(r)?),
        None => (JsValue::NULL, JsValue::NULL),
    };
    let out = Object::new();
    set(&out, "saveInfo", &JsValue::from_str(&inspection.save_info))?;
    set(&out, "webp", &webp)?;
    set(&out, "thumbnail", &thumbnail)?;
    Ok(out.into())
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
    let _ = Reflect::set(&err, &JsValue::from_str("code"), &JsValue::from_str(code.as_str()));
    err.into()
}
