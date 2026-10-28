//! A parser for the Lua subset the WoW client writes into SavedVariables:
//! `Name = value` statements whose values are tables (`["key"] = value,`,
//! `name = value,` or positional `value, -- [1]`), quoted strings with
//! backslash escapes, long-bracket strings, decimal numbers, booleans and
//! `nil`. Anything else (function calls, operators, hex numbers) is refused
//! with a typed error. Recursion is bounded by [`MAX_DEPTH`].

use crate::error::WowSvError;

/// Deepest table nesting accepted. WoW SavedVariables rarely pass 10.
pub(crate) const MAX_DEPTH: usize = 64;

/// A table key.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LuaKey {
    Str(String),
    Num(f64),
    Bool(bool),
}

/// A parsed value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LuaValue {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    Table(Vec<(LuaKey, LuaValue)>),
}

impl LuaValue {
    /// The value under a string key (the last one wins, as in Lua).
    pub(crate) fn get(&self, key: &str) -> Option<&LuaValue> {
        match self {
            LuaValue::Table(fields) => fields
                .iter()
                .rev()
                .find(|(k, _)| matches!(k, LuaKey::Str(s) if s == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// A short type name for error messages.
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            LuaValue::Nil => "nil",
            LuaValue::Bool(_) => "boolean",
            LuaValue::Num(_) => "number",
            LuaValue::Str(_) => "string",
            LuaValue::Table(_) => "table",
        }
    }
}

/// Parse every top-level `Name = value` statement.
pub(crate) fn parse_assignments(src: &[u8]) -> Result<Vec<(String, LuaValue)>, WowSvError> {
    let mut p = Parser { s: src, pos: 0 };
    if src.starts_with(&[0xEF, 0xBB, 0xBF]) {
        p.pos = 3;
    }
    let mut out = Vec::new();
    loop {
        p.skip_trivia()?;
        if p.peek().is_none() {
            return Ok(out);
        }
        let name = p
            .name()
            .ok_or_else(|| p.unexpected("a top-level variable name"))?;
        p.skip_trivia()?;
        p.expect(b'=', "a top-level assignment")?;
        p.skip_trivia()?;
        let value = p.value(0)?;
        out.push((name, value));
        p.skip_trivia()?;
        if p.peek() == Some(b';') {
            p.pos += 1;
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.s.get(self.pos + ahead).copied()
    }

    fn truncated(&self, what: &'static str) -> WowSvError {
        WowSvError::Truncated {
            what,
            offset: self.pos,
        }
    }

    fn unexpected(&self, what: &'static str) -> WowSvError {
        match self.peek() {
            None => self.truncated(what),
            Some(b) => WowSvError::Unexpected {
                what,
                found: if b.is_ascii_graphic() {
                    format!("'{}'", b as char)
                } else {
                    format!("byte 0x{b:02x}")
                },
                offset: self.pos,
            },
        }
    }

    fn expect(&mut self, byte: u8, what: &'static str) -> Result<(), WowSvError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.unexpected(what))
        }
    }

    /// Skip whitespace, `-- line` comments and `--[[ long ]]` comments.
    fn skip_trivia(&mut self) -> Result<(), WowSvError> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c) => self.pos += 1,
                Some(b'-') if self.peek_at(1) == Some(b'-') => {
                    self.pos += 2;
                    if self.peek() == Some(b'[') && self.long_bracket_level().is_some() {
                        self.long_bracket("a long comment")?;
                    } else {
                        while let Some(b) = self.peek() {
                            if b == b'\n' {
                                break;
                            }
                            self.pos += 1;
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// An identifier, or `None` (position unchanged) if there is none.
    fn name(&mut self) -> Option<String> {
        let start = self.pos;
        match self.peek() {
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
            _ => return None,
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Some(String::from_utf8_lossy(&self.s[start..self.pos]).into_owned())
    }

    fn value(&mut self, depth: usize) -> Result<LuaValue, WowSvError> {
        match self.peek() {
            None => Err(self.truncated("a value")),
            Some(b'{') => self.table(depth),
            Some(q @ (b'"' | b'\'')) => self.quoted(q).map(LuaValue::Str),
            Some(b'[') if self.long_bracket_level().is_some() => {
                self.long_bracket("a long string").map(LuaValue::Str)
            }
            Some(b'-' | b'.' | b'0'..=b'9') => self.number().map(LuaValue::Num),
            Some(_) => {
                let start = self.pos;
                match self.name().as_deref() {
                    Some("true") => Ok(LuaValue::Bool(true)),
                    Some("false") => Ok(LuaValue::Bool(false)),
                    Some("nil") => Ok(LuaValue::Nil),
                    _ => {
                        self.pos = start;
                        Err(self.unexpected("a value"))
                    }
                }
            }
        }
    }

    fn table(&mut self, depth: usize) -> Result<LuaValue, WowSvError> {
        if depth >= MAX_DEPTH {
            return Err(WowSvError::TooDeep {
                limit: MAX_DEPTH,
                offset: self.pos,
            });
        }
        self.pos += 1; // '{'
        let mut fields = Vec::new();
        let mut next_index = 1.0_f64;
        loop {
            self.skip_trivia()?;
            match self.peek() {
                None => return Err(self.truncated("a table")),
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(LuaValue::Table(fields));
                }
                _ => {}
            }
            let field = self.field(depth, &mut next_index)?;
            fields.push(field);
            self.skip_trivia()?;
            match self.peek() {
                Some(b',' | b';') => self.pos += 1,
                Some(b'}') => {}
                _ => return Err(self.unexpected("a table field separator")),
            }
        }
    }

    fn field(
        &mut self,
        depth: usize,
        next_index: &mut f64,
    ) -> Result<(LuaKey, LuaValue), WowSvError> {
        if self.peek() == Some(b'[') && self.long_bracket_level().is_none() {
            self.pos += 1;
            self.skip_trivia()?;
            let key_at = self.pos;
            let key = to_key(self.value(depth + 1)?, key_at)?;
            self.skip_trivia()?;
            self.expect(b']', "a table key")?;
            self.skip_trivia()?;
            self.expect(b'=', "a table field")?;
            self.skip_trivia()?;
            return Ok((key, self.value(depth + 1)?));
        }
        let start = self.pos;
        if let Some(name) = self.name() {
            self.skip_trivia()?;
            if self.peek() == Some(b'=') && self.peek_at(1) != Some(b'=') {
                self.pos += 1;
                self.skip_trivia()?;
                return Ok((LuaKey::Str(name), self.value(depth + 1)?));
            }
            self.pos = start;
        }
        let value = self.value(depth + 1)?;
        let key = LuaKey::Num(*next_index);
        *next_index += 1.0;
        Ok((key, value))
    }

    /// At `[`: the level of a long bracket opener (`[[` = 0, `[==[` = 2).
    fn long_bracket_level(&self) -> Option<usize> {
        let mut level = 0;
        loop {
            match self.peek_at(1 + level) {
                Some(b'=') => level += 1,
                Some(b'[') => return Some(level),
                _ => return None,
            }
        }
    }

    fn long_bracket(&mut self, what: &'static str) -> Result<String, WowSvError> {
        let level = self.long_bracket_level().unwrap_or(0);
        self.pos += level + 2;
        // A newline straight after the opener is not part of the string.
        match (self.peek(), self.peek_at(1)) {
            (Some(b'\r'), Some(b'\n')) | (Some(b'\n'), Some(b'\r')) => self.pos += 2,
            (Some(b'\r' | b'\n'), _) => self.pos += 1,
            _ => {}
        }
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == b']' {
                let closes = (1..=level).all(|i| self.peek_at(i) == Some(b'='))
                    && self.peek_at(level + 1) == Some(b']');
                if closes {
                    let text = String::from_utf8_lossy(&self.s[start..self.pos]).into_owned();
                    self.pos += level + 2;
                    return Ok(text);
                }
            }
            self.pos += 1;
        }
        Err(self.truncated(what))
    }

    fn quoted(&mut self, quote: u8) -> Result<String, WowSvError> {
        self.pos += 1;
        let mut buf = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.truncated("a string")),
                Some(b'\n' | b'\r') => return Err(self.unexpected("a string (raw newline)")),
                Some(b) if b == quote => {
                    self.pos += 1;
                    return Ok(String::from_utf8_lossy(&buf).into_owned());
                }
                Some(b'\\') => self.escape(&mut buf)?,
                Some(b) => {
                    buf.push(b);
                    self.pos += 1;
                }
            }
        }
    }

    fn escape(&mut self, buf: &mut Vec<u8>) -> Result<(), WowSvError> {
        let at = self.pos;
        self.pos += 1; // '\'
        let Some(c) = self.peek() else {
            return Err(self.truncated("an escape sequence"));
        };
        self.pos += 1;
        let simple = match c {
            b'n' => Some(b'\n'),
            b't' => Some(b'\t'),
            b'r' => Some(b'\r'),
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'v' => Some(0x0b),
            b'\\' | b'"' | b'\'' => Some(c),
            _ => None,
        };
        if let Some(byte) = simple {
            buf.push(byte);
            return Ok(());
        }
        match c {
            // Backslash + real newline is a newline (Lua; older WoW clients).
            b'\n' | b'\r' => {
                let pair = if c == b'\n' { b'\r' } else { b'\n' };
                if self.peek() == Some(pair) {
                    self.pos += 1;
                }
                buf.push(b'\n');
            }
            b'z' => {
                while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)) {
                    self.pos += 1;
                }
            }
            b'x' => {
                let hi = self.peek().and_then(hex_digit);
                let lo = self.peek_at(1).and_then(hex_digit);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        buf.push(h * 16 + l);
                        self.pos += 2;
                    }
                    _ => return Err(WowSvError::BadEscape { offset: at }),
                }
            }
            b'0'..=b'9' => {
                let mut n = u32::from(c - b'0');
                for _ in 0..2 {
                    match self.peek() {
                        Some(d @ b'0'..=b'9') => {
                            n = n * 10 + u32::from(d - b'0');
                            self.pos += 1;
                        }
                        _ => break,
                    }
                }
                let byte = u8::try_from(n).map_err(|_| WowSvError::BadEscape { offset: at })?;
                buf.push(byte);
            }
            _ => return Err(WowSvError::BadEscape { offset: at }),
        }
        Ok(())
    }

    fn number(&mut self) -> Result<f64, WowSvError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            let exp_sign =
                (b == b'+' || b == b'-') && matches!(
                    self.pos.checked_sub(1).and_then(|i| self.s.get(i)),
                    Some(b'e' | b'E')
                );
            if b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'E' || exp_sign {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = String::from_utf8_lossy(&self.s[start..self.pos]).into_owned();
        match text.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(n),
            _ => Err(WowSvError::BadNumber {
                text,
                offset: start,
            }),
        }
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn to_key(value: LuaValue, offset: usize) -> Result<LuaKey, WowSvError> {
    match value {
        LuaValue::Str(s) => Ok(LuaKey::Str(s)),
        LuaValue::Num(n) if !n.is_nan() => Ok(LuaKey::Num(n)),
        LuaValue::Bool(b) => Ok(LuaKey::Bool(b)),
        _ => Err(WowSvError::BadKey { offset }),
    }
}
