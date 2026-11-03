//! Typed errors. Every failure mode of the parsers is a variant here; no
//! parser panics on bad input.

use thiserror::Error;

/// Failure reading an LSPK package or one of its entries.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum LspkError {
    /// The input ends before a structure the header points at.
    #[error("package truncated: {what} needs bytes {start}..{end} but the input is {len} bytes")]
    Truncated {
        what: &'static str,
        start: u64,
        end: u64,
        len: u64,
    },
    /// The first four bytes are not `LSPK`.
    #[error("not an LSPK package (magic {0:02x?})")]
    BadMagic([u8; 4]),
    /// Only LSPK v18 (BG3) is supported.
    #[error("unsupported LSPK version {0} (only 18 is supported)")]
    UnsupportedVersion(u32),
    /// Saves are single-part; multi-part packages are not supported.
    #[error("multi-part packages are not supported (NumParts = {0})")]
    UnsupportedMultiPart(u16),
    /// The file table claims more entries than the reader accepts.
    #[error("file table lists {0} files, more than the 65536 limit")]
    TooManyFiles(u32),
    /// The LZ4 file table did not decompress to `NumFiles × 272` bytes.
    #[error("file table could not be decompressed: {0}")]
    FileTable(String),
    /// The entry lives in another archive part.
    #[error("entry {name:?} lives in archive part {part}, which is not supported")]
    UnsupportedArchivePart { name: String, part: u8 },
    /// The entry uses zlib or an unknown compression method.
    #[error("entry {name:?} uses unsupported compression method {method}")]
    UnsupportedCompression { name: String, method: u8 },
    /// The entry is larger than the caller's limit.
    #[error("entry {name:?} is {size} bytes, over the {limit}-byte limit")]
    EntryTooLarge { name: String, size: u64, limit: u64 },
    /// The compressed entry data is corrupt.
    #[error("entry {name:?} could not be decompressed: {reason}")]
    Decompress { name: String, reason: String },
    /// The entry decompressed to a different size than the table says.
    #[error("entry {name:?} decompressed to {actual} bytes, expected {expected}")]
    SizeMismatch {
        name: String,
        expected: u64,
        actual: u64,
    },
    /// No entry with that name.
    #[error("no entry named {0:?} in the package")]
    NotFound(String),
}

/// Failure reading an LSF v7 resource (`meta.lsf`). Callers that only want
/// the GameID treat every variant as "unknown layout" and fall back.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum LsfError {
    /// The first four bytes are not `LSOF`.
    #[error("not an LSF resource (magic {0:02x?})")]
    BadMagic([u8; 4]),
    /// Only LSF v7 (BG3) is supported.
    #[error("unsupported LSF version {0} (only 7 is supported)")]
    UnsupportedVersion(u32),
    /// The input ends before a structure the header points at.
    #[error("LSF truncated: {what} needs {need} bytes but the input is {have} bytes")]
    Truncated {
        what: &'static str,
        need: u64,
        have: u64,
    },
    /// zlib or an unknown compression method (low nibble of the flags).
    #[error("LSF uses unsupported compression method {0}")]
    UnsupportedCompression(u8),
    /// A section declares more decompressed bytes than the reader accepts.
    #[error("LSF {section} section is {size} bytes, over the {limit}-byte limit")]
    SectionTooLarge {
        section: &'static str,
        size: u64,
        limit: u64,
    },
    /// A section is corrupt: bad compressed data or a malformed record.
    #[error("LSF {section} section could not be decoded: {reason}")]
    Decode {
        section: &'static str,
        reason: String,
    },
    /// An attribute name points outside the string table.
    #[error("LSF name reference {0:#010x} is not in the string table")]
    BadStringRef(u32),
}

/// Failure reading a WebP header.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum WebpError {
    /// Not a `RIFF....WEBP` container.
    #[error("not a RIFF/WEBP container")]
    NotWebp,
    /// The header ends before the dimensions.
    #[error("WebP header truncated")]
    Truncated,
    /// The first chunk is not `VP8 `, `VP8L` or `VP8X`.
    #[error("unsupported WebP chunk {0:?}")]
    UnsupportedChunk(String),
    /// A `VP8 ` chunk without the `9d 01 2a` start code.
    #[error("bad VP8 start code")]
    BadVp8,
    /// A `VP8L` chunk without the `0x2f` signature.
    #[error("bad VP8L signature")]
    BadVp8l,
    /// A zero width or height.
    #[error("WebP has a zero dimension ({width}x{height})")]
    ZeroDimension { width: u32, height: u32 },
    /// Larger than the contract's blob cap.
    #[error("WebP is {size} bytes, over the {limit}-byte limit")]
    TooLarge { size: u64, limit: u64 },
}

/// Failure extracting `SaveInfo.json` or the thumbnail from a package.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExtractError {
    /// The package or the entry could not be read.
    #[error(transparent)]
    Lspk(#[from] LspkError),
    /// The package has no root `SaveInfo.json`.
    #[error("the package has no SaveInfo.json")]
    MissingSaveInfo,
    /// `SaveInfo.json` is not valid UTF-8 JSON.
    #[error("SaveInfo.json is not valid JSON: {0}")]
    SaveInfoJson(#[from] serde_json::Error),
    /// The package has no root `*.WebP` entry.
    #[error("the package has no WebP thumbnail")]
    MissingThumbnail,
    /// The thumbnail entry is not a readable WebP.
    #[error("the thumbnail is not a usable WebP: {0}")]
    InvalidWebp(#[from] WebpError),
}

/// Failure normalising `SaveInfo.json` into the `bg3-save/v1` payload.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NormaliseError {
    /// A key the normaliser reads is missing or has the wrong type.
    #[error("SaveInfo.json has an unexpected shape: {0}")]
    Shape(#[from] serde_json::Error),
    /// `Difficulty` must hold 1 to 4 strings.
    #[error("Difficulty must have 1 to 4 entries, found {0}")]
    Difficulty(usize),
    /// The party is empty (the contract requires at least one member).
    #[error("the active party is empty")]
    EmptyParty,
    /// A member has `Classes: []`; contract §3.2 says skip the save.
    #[error("party member {0} has an empty Classes list; the save must be skipped")]
    EmptyClasses(usize),
}

/// Failure parsing a WoW SavedVariables file (`LedgerLink.lua`).
///
/// Offsets are byte offsets into the input.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum WowSvError {
    /// The input is larger than the parser accepts.
    #[error("SavedVariables file is {size} bytes, over the {limit}-byte limit")]
    TooLarge { size: u64, limit: u64 },
    /// The input ends inside a statement, table, string or comment.
    #[error("SavedVariables truncated: input ends inside {what} at byte {offset}")]
    Truncated { what: &'static str, offset: usize },
    /// A byte that is not part of the supported Lua subset.
    #[error("unexpected {found} at byte {offset} while reading {what}")]
    Unexpected {
        what: &'static str,
        found: String,
        offset: usize,
    },
    /// A backslash escape the parser does not accept.
    #[error("invalid escape sequence in a string at byte {offset}")]
    BadEscape { offset: usize },
    /// A numeric literal that is not a finite decimal number.
    #[error("invalid number {text:?} at byte {offset}")]
    BadNumber { text: String, offset: usize },
    /// A table key that is `nil`, a table or NaN.
    #[error("invalid table key at byte {offset}")]
    BadKey { offset: usize },
    /// Tables nested deeper than the parser accepts.
    #[error("tables nested deeper than {limit} levels at byte {offset}")]
    TooDeep { limit: usize, offset: usize },
    /// `LedgerLinkDB` or `LedgerLinkDB.outbox` exists but is not a table.
    #[error("{what} is a {found}, not a table")]
    NotATable {
        what: &'static str,
        found: &'static str,
    },
}
