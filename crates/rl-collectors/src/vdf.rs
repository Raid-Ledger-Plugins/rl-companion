//! Valve KeyValues ("VDF") text parser, for Steam's `libraryfolders.vdf`
//! and `appmanifest_<appid>.acf` (ROK-2081 BG3 install discovery).
//!
//! Supports quoted keys and values (escapes `\\`, `\"`, `\n`, `\t`; any
//! other `\x` is kept as written), bare tokens, nested `{ }` objects and
//! `//` line comments. `[$PLATFORM]` conditionals are dropped (Steam's own
//! files do not use them). Keys compare ASCII case-insensitively, as Steam
//! does. Input is capped at [`VDF_MAX_BYTES`], nesting at [`VDF_MAX_DEPTH`].
//! Pure text in, typed values out — the caller reads the files.

use crate::error::VdfError;

/// Largest VDF text accepted (Steam's files are a few KB).
pub const VDF_MAX_BYTES: usize = 1024 * 1024;
/// Deepest object nesting accepted (`libraryfolders.vdf` uses 3).
pub const VDF_MAX_DEPTH: usize = 8;

/// A value: a string or a nested object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VdfValue {
    Str(String),
    Obj(VdfObject),
}

/// An object: key/value pairs in file order (duplicate keys kept).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VdfObject(pub Vec<(String, VdfValue)>);

impl VdfObject {
    /// The first value under `key`, ignoring ASCII case.
    pub fn get(&self, key: &str) -> Option<&VdfValue> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    /// The first string value under `key`.
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(VdfValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// The first object value under `key`.
    pub fn obj(&self, key: &str) -> Option<&VdfObject> {
        match self.get(key) {
            Some(VdfValue::Obj(o)) => Some(o),
            _ => None,
        }
    }
}

/// Parse a whole VDF document into its top-level object.
pub fn parse_vdf(text: &str) -> Result<VdfObject, VdfError> {
    if text.len() > VDF_MAX_BYTES {
        return Err(VdfError::TooLarge {
            size: text.len(),
            limit: VDF_MAX_BYTES,
        });
    }
    let mut lexer = Lexer {
        bytes: text.as_bytes(),
        at: 0,
        line: 1,
    };
    parse_object(&mut lexer, 0)
}

/// One Steam library from `libraryfolders.vdf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamLibrary {
    /// The library root as written (Windows paths unescaped).
    pub path: String,
    /// App ids installed there (empty for the pre-2021 format).
    pub apps: Vec<String>,
}

/// The libraries in a `libraryfolders.vdf`, current (`"0" { "path" … "apps"
/// { … } }`) or pre-2021 (`"1" "D:\\SteamLibrary"`) format. Entries with no
/// path are skipped.
pub fn library_paths(text: &str) -> Result<Vec<SteamLibrary>, VdfError> {
    let doc = parse_vdf(text)?;
    let root = doc
        .obj("libraryfolders")
        .ok_or(VdfError::MissingKey("libraryfolders"))?;
    let mut out = Vec::new();
    for (key, value) in &root.0 {
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let (path, apps) = match value {
            VdfValue::Str(path) => (path.as_str(), Vec::new()),
            VdfValue::Obj(lib) => {
                let apps = lib
                    .obj("apps")
                    .map(|a| a.0.iter().map(|(id, _)| id.clone()).collect());
                (lib.str("path").unwrap_or(""), apps.unwrap_or_default())
            }
        };
        if !path.is_empty() {
            out.push(SteamLibrary {
                path: path.to_owned(),
                apps,
            });
        }
    }
    Ok(out)
}

/// The fields of an `appmanifest_<appid>.acf` the catalogue needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppManifest {
    pub appid: Option<String>,
    /// Folder name under `steamapps/common/`; a single path component.
    pub installdir: String,
    /// Steam build id; changes on every update.
    pub buildid: Option<String>,
}

/// Read an `appmanifest_*.acf`. `installdir` is required and must be one
/// plain path component (no separators, drive colon, `.`/`..` or NUL), so
/// it can never steer the caller outside `steamapps/common/`.
pub fn app_manifest(text: &str) -> Result<AppManifest, VdfError> {
    let doc = parse_vdf(text)?;
    let state = doc
        .obj("AppState")
        .ok_or(VdfError::MissingKey("AppState"))?;
    let installdir = state
        .str("installdir")
        .ok_or(VdfError::MissingKey("installdir"))?;
    let unsafe_char = |c: char| matches!(c, '/' | '\\' | ':' | '\0');
    if installdir.is_empty()
        || installdir == "."
        || installdir == ".."
        || installdir.contains(unsafe_char)
    {
        return Err(VdfError::BadValue {
            key: "installdir",
            reason: "not a single folder name",
        });
    }
    let owned = |k: &str| state.str(k).filter(|v| !v.is_empty()).map(str::to_owned);
    Ok(AppManifest {
        appid: owned("appid"),
        installdir: installdir.to_owned(),
        buildid: owned("buildid"),
    })
}

enum Token {
    Str(String),
    Open,
    Close,
    Eof,
}

/// Parse pairs until `}` (when `depth > 0`) or end of input (top level).
fn parse_object(lx: &mut Lexer<'_>, depth: usize) -> Result<VdfObject, VdfError> {
    let mut pairs = Vec::new();
    loop {
        let key = match lx.next()? {
            Token::Str(key) => key,
            Token::Close if depth > 0 => return Ok(VdfObject(pairs)),
            Token::Eof if depth == 0 => return Ok(VdfObject(pairs)),
            Token::Close => return Err(lx.syntax("unmatched '}'")),
            Token::Eof => return Err(lx.syntax("unterminated object")),
            Token::Open => return Err(lx.syntax("'{' where a key was expected")),
        };
        let value = match lx.next()? {
            Token::Str(v) => VdfValue::Str(v),
            Token::Open if depth < VDF_MAX_DEPTH => VdfValue::Obj(parse_object(lx, depth + 1)?),
            Token::Open => {
                return Err(VdfError::TooDeep {
                    line: lx.line,
                    limit: VDF_MAX_DEPTH,
                });
            }
            Token::Close | Token::Eof => return Err(lx.syntax("key without a value")),
        };
        pairs.push((key, value));
    }
}

struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
    line: usize,
}

impl Lexer<'_> {
    fn syntax(&self, reason: &'static str) -> VdfError {
        VdfError::Syntax {
            line: self.line,
            reason,
        }
    }

    fn next(&mut self) -> Result<Token, VdfError> {
        loop {
            self.skip_space_and_comments();
            let Some(&b) = self.bytes.get(self.at) else {
                return Ok(Token::Eof);
            };
            match b {
                b'{' => return Ok(self.single(Token::Open)),
                b'}' => return Ok(self.single(Token::Close)),
                b'"' => return self.quoted().map(Token::Str),
                _ => {
                    let bare = self.bare();
                    let conditional = bare.starts_with('[') && bare.ends_with(']');
                    if !conditional {
                        return Ok(Token::Str(bare));
                    }
                }
            }
        }
    }

    fn single(&mut self, token: Token) -> Token {
        self.at += 1;
        token
    }

    fn skip_space_and_comments(&mut self) {
        while let Some(&b) = self.bytes.get(self.at) {
            if b == b'/' && self.bytes.get(self.at + 1) == Some(&b'/') {
                while self.bytes.get(self.at).is_some_and(|&c| c != b'\n') {
                    self.at += 1;
                }
            } else if b.is_ascii_whitespace() {
                self.line += usize::from(b == b'\n');
                self.at += 1;
            } else {
                return;
            }
        }
    }

    fn bare(&mut self) -> String {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|&b| !b.is_ascii_whitespace() && !matches!(b, b'{' | b'}' | b'"'))
        {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned()
    }

    /// A `"…"` string. Byte-wise is safe: every special byte is ASCII, and
    /// the input came from a `&str`, so the collected bytes stay UTF-8.
    fn quoted(&mut self) -> Result<String, VdfError> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let Some(&b) = self.bytes.get(self.at) else {
                return Err(self.syntax("unterminated string"));
            };
            self.at += 1;
            match b {
                b'"' => return Ok(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => match self.bytes.get(self.at) {
                    Some(&e @ (b'\\' | b'"' | b'n' | b't')) => {
                        self.at += 1;
                        out.push(match e {
                            b'n' => b'\n',
                            b't' => b'\t',
                            other => other,
                        });
                    }
                    _ => out.push(b'\\'),
                },
                _ => {
                    self.line += usize::from(b == b'\n');
                    out.push(b);
                }
            }
        }
    }
}
