//! Lexer of Lua 5.0 source (a port of `llex.c`).

use std::rc::Rc;

use super::CompileError;

/// A token. Single-character tokens are `Char`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Char(u8),
    And,
    Break,
    Do,
    Else,
    Elseif,
    End,
    False,
    For,
    Function,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,
    Name(Rc<[u8]>),
    Concat,
    Dots,
    Eq,
    Ge,
    Le,
    Ne,
    Number(f64),
    String(Rc<[u8]>),
    Eos,
}

const RESERVED: [(&[u8], Tok); 21] = [
    (b"and", Tok::And),
    (b"break", Tok::Break),
    (b"do", Tok::Do),
    (b"else", Tok::Else),
    (b"elseif", Tok::Elseif),
    (b"end", Tok::End),
    (b"false", Tok::False),
    (b"for", Tok::For),
    (b"function", Tok::Function),
    (b"if", Tok::If),
    (b"in", Tok::In),
    (b"local", Tok::Local),
    (b"nil", Tok::Nil),
    (b"not", Tok::Not),
    (b"or", Tok::Or),
    (b"repeat", Tok::Repeat),
    (b"return", Tok::Return),
    (b"then", Tok::Then),
    (b"true", Tok::True),
    (b"until", Tok::Until),
    (b"while", Tok::While),
];

/// True for reserved words of Lua 5.0.
pub fn is_reserved(word: &[u8]) -> bool {
    RESERVED.iter().any(|(w, _)| *w == word)
}

impl Tok {
    /// Text of a token kind for error messages (`luaX_token2str`).
    pub fn describe(&self) -> String {
        let fixed = match self {
            Tok::Char(c) => return (*c as char).to_string(),
            Tok::Name(n) => return String::from_utf8_lossy(n).into_owned(),
            Tok::Concat => "..",
            Tok::Dots => "...",
            Tok::Eq => "==",
            Tok::Ge => ">=",
            Tok::Le => "<=",
            Tok::Ne => "~=",
            Tok::Number(_) => "<number>",
            Tok::String(_) => "<string>",
            Tok::Eos => "<eof>",
            other => {
                return RESERVED
                    .iter()
                    .find(|(_, t)| t == other)
                    .map(|(w, _)| String::from_utf8_lossy(w).into_owned())
                    .unwrap_or_default();
            }
        };
        fixed.to_owned()
    }
}

pub(crate) struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    /// Line of the current character.
    pub line: u32,
    /// Raw text of the last number or string token, for error messages.
    pub raw: String,
    float: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a [u8], float: bool) -> Self {
        let mut lexer = Self {
            src,
            pos: 0,
            line: 1,
            raw: String::new(),
            float,
        };
        if lexer.current() == Some(b'#') {
            while !matches!(lexer.current(), Some(b'\n') | None) {
                lexer.pos += 1;
            }
        }
        lexer
    }

    fn current(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn raw_since(&self, start: usize) -> String {
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    fn newline(&mut self) {
        self.pos += 1;
        self.line += 1;
    }

    fn error(&self, msg: &str, near: &str) -> CompileError {
        CompileError {
            line: self.line,
            message: format!("{msg} near `{near}'"),
        }
    }

    fn read_numeral(&mut self, start: usize) -> Result<Tok, CompileError> {
        while self.current().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }
        if self.current() == Some(b'.') {
            self.advance();
            if self.current() == Some(b'.') {
                self.advance();
                let text = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                return Err(self.error(
                    "ambiguous syntax (decimal point x string concatenation)",
                    &text,
                ));
            }
        }
        while self.current().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }
        if matches!(self.current(), Some(b'e' | b'E')) {
            self.advance();
            if matches!(self.current(), Some(b'+' | b'-')) {
                self.advance();
            }
            while self.current().is_some_and(|c| c.is_ascii_digit()) {
                self.advance();
            }
        }
        let text = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        self.raw.clone_from(&text);
        let value = parse_number(&text).ok_or_else(|| self.error("malformed number", &text))?;
        Ok(Tok::Number(if self.float {
            value as f32 as f64
        } else {
            value
        }))
    }

    /// Reads `[[ ... ]]` after the first `[`; `None` for comments.
    fn read_long_string(&mut self, keep: bool) -> Result<Option<Vec<u8>>, CompileError> {
        self.advance(); // second '['
        if self.current() == Some(b'\n') {
            self.newline();
        }
        let mut out = Vec::new();
        let mut nesting = 0u32;
        loop {
            match self.current() {
                None => {
                    let what = if keep {
                        "unfinished long string"
                    } else {
                        "unfinished long comment"
                    };
                    return Err(self.error(what, "<eof>"));
                }
                Some(b'[') => {
                    out.push(b'[');
                    self.advance();
                    if self.current() == Some(b'[') {
                        nesting += 1;
                        out.push(b'[');
                        self.advance();
                    }
                }
                Some(b']') => {
                    self.advance();
                    if self.current() == Some(b']') {
                        if nesting == 0 {
                            self.advance();
                            break;
                        }
                        nesting -= 1;
                        out.push(b']');
                        self.advance();
                    }
                    out.push(b']');
                }
                Some(b'\n') => {
                    out.push(b'\n');
                    self.newline();
                }
                Some(c) => {
                    out.push(c);
                    self.advance();
                }
            }
        }
        Ok(keep.then_some(out))
    }

    fn read_string(&mut self, delimiter: u8) -> Result<Tok, CompileError> {
        let start = self.pos;
        self.advance();
        let mut out = Vec::new();
        loop {
            match self.current() {
                None => return Err(self.error("unfinished string", "<eof>")),
                Some(b'\n') => return Err(self.error("unfinished string", &self.raw_since(start))),
                Some(c) if c == delimiter => {
                    self.advance();
                    break;
                }
                Some(b'\\') => {
                    self.advance();
                    match self.current() {
                        Some(b'a') => out.push(7),
                        Some(b'b') => out.push(8),
                        Some(b'f') => out.push(12),
                        Some(b'n') => out.push(b'\n'),
                        Some(b'r') => out.push(b'\r'),
                        Some(b't') => out.push(b'\t'),
                        Some(b'v') => out.push(11),
                        Some(b'\n') => {
                            out.push(b'\n');
                            self.newline();
                            continue;
                        }
                        None => continue,
                        Some(c) if c.is_ascii_digit() => {
                            let mut value = 0u32;
                            let mut digits = 0;
                            while digits < 3 && self.current().is_some_and(|c| c.is_ascii_digit()) {
                                value = value * 10 + (self.current().unwrap_or(b'0') - b'0') as u32;
                                self.advance();
                                digits += 1;
                            }
                            if value > 255 {
                                return Err(
                                    self.error("escape sequence too large", &self.raw_since(start))
                                );
                            }
                            out.push(value as u8);
                            continue;
                        }
                        Some(c) => out.push(c),
                    }
                    self.advance();
                }
                Some(c) => {
                    out.push(c);
                    self.advance();
                }
            }
        }
        self.raw = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        Ok(Tok::String(out.into()))
    }

    /// Next token (`luaX_lex`).
    pub fn next_token(&mut self) -> Result<Tok, CompileError> {
        loop {
            let Some(c) = self.current() else {
                return Ok(Tok::Eos);
            };
            match c {
                b'\n' => self.newline(),
                b'-' => {
                    self.advance();
                    if self.current() != Some(b'-') {
                        return Ok(Tok::Char(b'-'));
                    }
                    self.advance();
                    if self.current() == Some(b'[') {
                        self.advance();
                        if self.current() == Some(b'[') {
                            self.read_long_string(false)?;
                            continue;
                        }
                    }
                    while !matches!(self.current(), Some(b'\n') | None) {
                        self.advance();
                    }
                }
                b'[' => {
                    self.advance();
                    if self.current() != Some(b'[') {
                        return Ok(Tok::Char(b'['));
                    }
                    let s = self.read_long_string(true)?.unwrap_or_default();
                    self.raw = String::from_utf8_lossy(&s).into_owned();
                    return Ok(Tok::String(s.into()));
                }
                b'=' | b'<' | b'>' | b'~' => {
                    self.advance();
                    if self.current() != Some(b'=') {
                        return Ok(Tok::Char(c));
                    }
                    self.advance();
                    return Ok(match c {
                        b'=' => Tok::Eq,
                        b'<' => Tok::Le,
                        b'>' => Tok::Ge,
                        _ => Tok::Ne,
                    });
                }
                b'"' | b'\'' => return self.read_string(c),
                b'.' => {
                    let start = self.pos;
                    self.advance();
                    if self.current() == Some(b'.') {
                        self.advance();
                        if self.current() == Some(b'.') {
                            self.advance();
                            return Ok(Tok::Dots);
                        }
                        return Ok(Tok::Concat);
                    }
                    if !self.current().is_some_and(|c| c.is_ascii_digit()) {
                        return Ok(Tok::Char(b'.'));
                    }
                    return self.read_numeral(start);
                }
                _ if c.is_ascii_whitespace() || c == 0x0B => self.advance(),
                _ if c.is_ascii_digit() => return self.read_numeral(self.pos),
                _ if c.is_ascii_alphabetic() || c == b'_' => {
                    let start = self.pos;
                    while self
                        .current()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
                    {
                        self.advance();
                    }
                    let word = &self.src[start..self.pos];
                    if let Some((_, tok)) = RESERVED.iter().find(|(w, _)| *w == word) {
                        return Ok(tok.clone());
                    }
                    return Ok(Tok::Name(word.into()));
                }
                _ => {
                    if c.is_ascii_control() {
                        return Err(self.error("invalid control char", &format!("char({c})")));
                    }
                    self.advance();
                    return Ok(Tok::Char(c));
                }
            }
        }
    }
}

/// `strtod` on the characters the lexer collects: digits, one point, an exponent.
fn parse_number(text: &str) -> Option<f64> {
    let mut s = text.to_owned();
    if s.starts_with('.') {
        s.insert(0, '0');
    }
    if let Some(dot) = s.find('.')
        && !s[dot + 1..].starts_with(|c: char| c.is_ascii_digit())
    {
        s.insert(dot + 1, '0');
    }
    s.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(src: &str) -> Vec<Tok> {
        let mut lexer = Lexer::new(src.as_bytes(), true);
        let mut out = Vec::new();
        loop {
            let t = lexer.next_token().unwrap();
            if t == Tok::Eos {
                return out;
            }
            out.push(t);
        }
    }

    #[test]
    fn reads_tokens() {
        assert_eq!(
            tokens("local x = 0.2 .. 'a\\65\\n' -- c\n[[\nb]]"),
            vec![
                Tok::Local,
                Tok::Name(b"x"[..].into()),
                Tok::Char(b'='),
                Tok::Number(0.2f32 as f64),
                Tok::Concat,
                Tok::String(b"aA\n"[..].into()),
                Tok::String(b"b"[..].into()),
            ]
        );
        assert_eq!(tokens("5. .5 1e3 ..."), {
            vec![
                Tok::Number(5.0),
                Tok::Number(0.5),
                Tok::Number(1000.0),
                Tok::Dots,
            ]
        });
        assert_eq!(tokens("--[[ x ]] a ~= b"), {
            vec![
                Tok::Name(b"a"[..].into()),
                Tok::Ne,
                Tok::Name(b"b"[..].into()),
            ]
        });
    }

    #[test]
    fn reports_errors_with_lines() {
        let mut lexer = Lexer::new(b"\n\n'abc", true);
        let err = lexer.next_token().unwrap_err();
        assert_eq!(err.line, 3);
        assert!(err.message.contains("unfinished string"), "{err}");
        assert!(Lexer::new(b"1..2", true).next_token().is_err());
    }
}
