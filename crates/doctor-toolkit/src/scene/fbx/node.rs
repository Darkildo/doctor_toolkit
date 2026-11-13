//! The FBX node tree: binary (any 7.x version, 32- or 64-bit offsets,
//! zlib-compressed arrays) and ASCII parsing, binary 7.4 writing.
//!
//! ```text
//! binary: "Kaydara FBX Binary  \0" 1A 00, u32 version
//! node:   u32 end, u32 property count, u32 property bytes, u8 name length, name
//!         (u64 for the first three from version 7500)
//!         properties, child nodes, a null record when there are children
//! property: 'Y' i16, 'C' bool, 'I' i32, 'F' f32, 'D' f64, 'L' i64,
//!           'S' string / 'R' bytes (u32 length),
//!           'f' 'd' 'l' 'i' 'b' arrays: u32 count, u32 encoding (1 = zlib), u32 bytes
//! ```

use std::io::Read as _;

use crate::{Error, Result};

/// A property of a node.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Bool(bool),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    /// Strings are bytes in FBX (`Name\0\x01Class` in binary files).
    String(Vec<u8>),
    Bytes(Vec<u8>),
    Bools(Vec<bool>),
    I32s(Vec<i32>),
    I64s(Vec<i64>),
    F32s(Vec<f32>),
    F64s(Vec<f64>),
}

impl Prop {
    pub fn str(s: &str) -> Self {
        Self::String(s.as_bytes().to_vec())
    }

    pub fn as_i64(&self) -> Option<i64> {
        Some(match self {
            Self::Bool(b) => *b as i64,
            Self::I16(v) => *v as i64,
            Self::I32(v) => *v as i64,
            Self::I64(v) => *v,
            Self::F32(v) => *v as i64,
            Self::F64(v) => *v as i64,
            Self::String(s) => std::str::from_utf8(s).ok()?.trim().parse().ok()?,
            _ => return None,
        })
    }

    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            Self::F32(v) => *v as f64,
            Self::F64(v) => *v,
            Self::String(s) => std::str::from_utf8(s).ok()?.trim().parse().ok()?,
            other => other.as_i64()? as f64,
        })
    }

    pub fn as_str(&self) -> Option<std::borrow::Cow<'_, str>> {
        match self {
            Self::String(s) | Self::Bytes(s) => Some(String::from_utf8_lossy(s)),
            _ => None,
        }
    }

    /// An array (or a single number) as doubles.
    pub fn f64s(&self) -> Option<Vec<f64>> {
        Some(match self {
            Self::F64s(v) => v.clone(),
            Self::F32s(v) => v.iter().map(|&x| x as f64).collect(),
            Self::I32s(v) => v.iter().map(|&x| x as f64).collect(),
            Self::I64s(v) => v.iter().map(|&x| x as f64).collect(),
            Self::Bools(v) => v.iter().map(|&x| x as u8 as f64).collect(),
            other => vec![other.as_f64()?],
        })
    }

    /// An array (or a single number) as integers.
    pub fn i64s(&self) -> Option<Vec<i64>> {
        Some(match self {
            Self::I32s(v) => v.iter().map(|&x| x as i64).collect(),
            Self::I64s(v) => v.clone(),
            Self::F64s(v) => v.iter().map(|&x| x as i64).collect(),
            Self::F32s(v) => v.iter().map(|&x| x as i64).collect(),
            Self::Bools(v) => v.iter().map(|&x| x as i64).collect(),
            other => vec![other.as_i64()?],
        })
    }
}

/// A node: name, properties and children.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Node {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: &str, props: Vec<Prop>) -> Self {
        Self {
            name: name.to_owned(),
            props,
            children: Vec::new(),
        }
    }

    pub fn with(mut self, children: Vec<Node>) -> Self {
        self.children = children;
        self
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The first property of a child.
    pub fn value(&self, name: &str) -> Option<&Prop> {
        self.child(name).and_then(|c| c.props.first())
    }

    /// The array of a child (all its properties joined for ASCII files of
    /// the old form `Name: 1,2,3`).
    pub fn array_f64(&self, name: &str) -> Option<Vec<f64>> {
        let c = self.child(name)?;
        if c.props.len() == 1 {
            return c.props[0].f64s();
        }
        c.props.iter().map(Prop::as_f64).collect()
    }

    pub fn array_i64(&self, name: &str) -> Option<Vec<i64>> {
        let c = self.child(name)?;
        if c.props.len() == 1 {
            return c.props[0].i64s();
        }
        c.props.iter().map(Prop::as_i64).collect()
    }

    pub fn string(&self, name: &str) -> Option<String> {
        self.value(name)
            .and_then(Prop::as_str)
            .map(|s| s.into_owned())
    }
}

/// A parsed file: its version and top-level nodes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub version: u32,
    pub binary: bool,
    pub nodes: Vec<Node>,
}

impl Document {
    pub fn get(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }
}

pub const BINARY_MAGIC: &[u8] = b"Kaydara FBX Binary  \0";

/// Largest decoded array (elements) and nesting accepted.
const MAX_ARRAY: usize = 1 << 28;
const MAX_DEPTH: usize = 64;

fn bad(message: impl Into<String>) -> Error {
    Error::InvalidArgument(format!("FBX: {}", message.into()))
}

/// Parses a binary or ASCII FBX file.
pub fn parse(bytes: &[u8]) -> Result<Document> {
    if bytes.starts_with(BINARY_MAGIC) {
        parse_binary(bytes)
    } else {
        parse_ascii(bytes)
    }
}

struct Binary<'a> {
    data: &'a [u8],
    wide: bool,
    /// Bytes of decompressed arrays so far (bounds memory use).
    inflated: usize,
}

impl<'a> Binary<'a> {
    fn slice(&self, at: usize, len: usize) -> Result<&'a [u8]> {
        at.checked_add(len)
            .and_then(|end| self.data.get(at..end))
            .ok_or_else(|| bad(format!("truncated at {at:#x}")))
    }

    fn u8(&self, at: usize) -> Result<u8> {
        Ok(self.slice(at, 1)?[0])
    }

    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.slice(at, 4)?.try_into().unwrap()))
    }

    fn u64(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.slice(at, 8)?.try_into().unwrap()))
    }

    fn word(&self, at: usize) -> Result<u64> {
        if self.wide {
            self.u64(at)
        } else {
            self.u32(at).map(u64::from)
        }
    }

    /// Reads a node at `at`; `None` for the null record. Returns the end.
    fn node(&mut self, at: usize, depth: usize) -> Result<(Option<Node>, usize)> {
        if depth > MAX_DEPTH {
            return Err(bad("nodes nested too deeply"));
        }
        let w = if self.wide { 8 } else { 4 };
        let end = self.word(at)? as usize;
        let count = self.word(at + w)? as usize;
        let _props_len = self.word(at + 2 * w)?;
        let name_len = self.u8(at + 3 * w)? as usize;
        let header = 3 * w + 1;
        if end == 0 {
            return Ok((None, at + header + name_len));
        }
        if end <= at || end > self.data.len() {
            return Err(bad(format!("node at {at:#x} ends at {end:#x}")));
        }
        let name = String::from_utf8_lossy(self.slice(at + header, name_len)?).into_owned();
        let mut pos = at + header + name_len;
        // A name that runs past the end of the node would make `end - pos`
        // wrap around and let any count through (a fuzzed file asked for
        // 18 GB of properties).
        let room = end
            .checked_sub(pos)
            .ok_or_else(|| bad(format!("node `{name}` ends inside its name")))?;
        if count > room {
            return Err(bad(format!("node `{name}`: {count} properties")));
        }
        let mut props = Vec::with_capacity(count);
        for _ in 0..count {
            let (prop, next) = self.prop(pos)?;
            if next > end {
                return Err(bad(format!("node `{name}`: properties overrun")));
            }
            props.push(prop);
            pos = next;
        }
        let mut children = Vec::new();
        while pos < end {
            let (child, next) = self.node(pos, depth + 1)?;
            pos = next;
            match child {
                Some(c) => children.push(c),
                None => break,
            }
        }
        Ok((
            Some(Node {
                name,
                props,
                children,
            }),
            end,
        ))
    }

    fn prop(&mut self, at: usize) -> Result<(Prop, usize)> {
        let code = self.u8(at)?;
        let p = at + 1;
        let fixed = |len: usize| -> Result<&'a [u8]> { self.slice(p, len) };
        Ok(match code {
            b'Y' => (
                Prop::I16(i16::from_le_bytes(fixed(2)?.try_into().unwrap())),
                p + 2,
            ),
            b'C' => (Prop::Bool(fixed(1)?[0] != 0), p + 1),
            b'I' => (
                Prop::I32(i32::from_le_bytes(fixed(4)?.try_into().unwrap())),
                p + 4,
            ),
            b'F' => (
                Prop::F32(f32::from_le_bytes(fixed(4)?.try_into().unwrap())),
                p + 4,
            ),
            b'D' => (
                Prop::F64(f64::from_le_bytes(fixed(8)?.try_into().unwrap())),
                p + 8,
            ),
            b'L' => (
                Prop::I64(i64::from_le_bytes(fixed(8)?.try_into().unwrap())),
                p + 8,
            ),
            b'S' | b'R' => {
                let len = self.u32(p)? as usize;
                let bytes = self.slice(p + 4, len)?.to_vec();
                let prop = if code == b'S' {
                    Prop::String(bytes)
                } else {
                    Prop::Bytes(bytes)
                };
                (prop, p + 4 + len)
            }
            b'f' | b'd' | b'l' | b'i' | b'b' => {
                let count = self.u32(p)? as usize;
                let encoding = self.u32(p + 4)?;
                let stored = self.u32(p + 8)? as usize;
                let raw = self.slice(p + 12, stored)?;
                let size = match code {
                    b'f' | b'i' => 4,
                    b'd' | b'l' => 8,
                    _ => 1,
                };
                if count > MAX_ARRAY {
                    return Err(bad(format!("array of {count} elements")));
                }
                let want = count * size;
                let data: std::borrow::Cow<'_, [u8]> = match encoding {
                    0 => {
                        if stored < want {
                            return Err(bad("array shorter than its count"));
                        }
                        std::borrow::Cow::Borrowed(&raw[..want])
                    }
                    1 => {
                        // zlib packs at most ~1032:1; more is a damaged or hostile file.
                        let budget =
                            (self.data.len().saturating_mul(1100)).clamp(64 << 20, 1 << 31);
                        self.inflated += want;
                        if self.inflated > budget {
                            return Err(bad("compressed arrays too large for the file"));
                        }
                        let mut out = Vec::with_capacity(want);
                        flate2::read::ZlibDecoder::new(raw)
                            .take(want as u64)
                            .read_to_end(&mut out)
                            .map_err(|e| bad(format!("compressed array: {e}")))?;
                        if out.len() < want {
                            return Err(bad("compressed array shorter than its count"));
                        }
                        std::borrow::Cow::Owned(out)
                    }
                    other => return Err(bad(format!("array encoding {other}"))),
                };
                let prop = match code {
                    b'f' => Prop::F32s(
                        data.chunks_exact(4)
                            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                            .collect(),
                    ),
                    b'd' => Prop::F64s(
                        data.chunks_exact(8)
                            .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                            .collect(),
                    ),
                    b'l' => Prop::I64s(
                        data.chunks_exact(8)
                            .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
                            .collect(),
                    ),
                    b'i' => Prop::I32s(
                        data.chunks_exact(4)
                            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                            .collect(),
                    ),
                    _ => Prop::Bools(data.iter().map(|&b| b != 0).collect()),
                };
                (prop, p + 12 + stored)
            }
            other => {
                return Err(bad(format!(
                    "unknown property type {:?} at {at:#x}",
                    other as char
                )));
            }
        })
    }
}

fn parse_binary(bytes: &[u8]) -> Result<Document> {
    if bytes.len() < 27 {
        return Err(bad("file too short"));
    }
    let version = u32::from_le_bytes(bytes[23..27].try_into().unwrap());
    let mut reader = Binary {
        data: bytes,
        wide: version >= 7500,
        inflated: 0,
    };
    let mut nodes = Vec::new();
    let mut pos = 27;
    let w = if reader.wide { 8 } else { 4 };
    while pos + 3 * w < bytes.len() {
        let (node, next) = reader.node(pos, 0)?;
        match node {
            Some(n) => nodes.push(n),
            None => break,
        }
        pos = next;
    }
    if nodes.is_empty() {
        return Err(bad("no nodes"));
    }
    Ok(Document {
        version,
        binary: true,
        nodes,
    })
}

// ------------------------------------------------------------------ ASCII

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Key(String),
    Str(Vec<u8>),
    Number(String),
    Word(String),
    Comma,
    Open,
    Close,
    Star,
    Newline,
}

fn tokenize(text: &[u8]) -> Result<Vec<Token>> {
    let mut out = Vec::new();
    let mut i = 0;
    let n = text.len();
    while i < n {
        let c = text[i];
        match c {
            b';' => {
                while i < n && text[i] != b'\n' {
                    i += 1;
                }
            }
            b'\n' => {
                out.push(Token::Newline);
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b',' => {
                out.push(Token::Comma);
                i += 1;
            }
            b'{' => {
                out.push(Token::Open);
                i += 1;
            }
            b'}' => {
                out.push(Token::Close);
                i += 1;
            }
            b'*' => {
                out.push(Token::Star);
                i += 1;
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < n && text[i] != b'"' {
                    i += 1;
                }
                // "&quot;" is the FBX escape of a quote.
                let raw = &text[start..i.min(n)];
                let s = String::from_utf8_lossy(raw).replace("&quot;", "\"");
                out.push(Token::Str(s.into_bytes()));
                i += 1;
            }
            _ => {
                let start = i;
                while i < n
                    && !matches!(
                        text[i],
                        b' ' | b'\t' | b'\r' | b'\n' | b',' | b'{' | b'}' | b'"' | b';'
                    )
                {
                    if text[i] == b':' {
                        break;
                    }
                    i += 1;
                }
                let word = String::from_utf8_lossy(&text[start..i]).into_owned();
                if i < n && text[i] == b':' {
                    out.push(Token::Key(word));
                    i += 1;
                } else if word
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_digit() || b == b'-' || b == b'+' || b == b'.')
                {
                    out.push(Token::Number(word));
                } else if word.is_empty() {
                    i += 1;
                } else {
                    out.push(Token::Word(word));
                }
            }
        }
        if out.len() > MAX_ARRAY {
            return Err(bad("too many tokens"));
        }
    }
    Ok(out)
}

fn number(word: &str) -> Prop {
    if !word.contains(['.', 'e', 'E'])
        && let Ok(v) = word.parse::<i64>()
    {
        return if let Ok(v) = i32::try_from(v) {
            Prop::I32(v)
        } else {
            Prop::I64(v)
        };
    }
    match word.parse::<f64>() {
        Ok(v) => Prop::F64(v),
        Err(_) => Prop::String(word.as_bytes().to_vec()),
    }
}

struct Ascii {
    tokens: Vec<Token>,
    pos: usize,
}

impl Ascii {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    /// Nodes until `}` or the end.
    fn nodes(&mut self, depth: usize) -> Result<Vec<Node>> {
        if depth > MAX_DEPTH {
            return Err(bad("nodes nested too deeply"));
        }
        let mut out = Vec::new();
        while let Some(token) = self.peek().cloned() {
            match token {
                Token::Close => {
                    self.pos += 1;
                    break;
                }
                Token::Key(name) => {
                    self.pos += 1;
                    out.push(self.node(name, depth)?);
                }
                _ => self.pos += 1,
            }
        }
        Ok(out)
    }

    fn node(&mut self, name: String, depth: usize) -> Result<Node> {
        let mut node = Node {
            name,
            ..Default::default()
        };
        // Properties until a newline that does not follow a comma, or `{`.
        let mut after_comma = false;
        while let Some(token) = self.peek().cloned() {
            match token {
                Token::Newline => {
                    self.pos += 1;
                    if !after_comma && !node.props.is_empty() {
                        break;
                    }
                    if node.props.is_empty() && !after_comma {
                        // `Name:` alone on its line: maybe `{` follows.
                        if !matches!(self.peek(), Some(Token::Open)) {
                            break;
                        }
                    }
                }
                Token::Comma => {
                    after_comma = true;
                    self.pos += 1;
                    continue;
                }
                Token::Str(s) => {
                    node.props.push(Prop::String(s));
                    self.pos += 1;
                }
                Token::Number(w) => {
                    node.props.push(number(&w));
                    self.pos += 1;
                }
                Token::Word(w) => {
                    node.props.push(match w.as_str() {
                        "T" | "Y" => Prop::Bool(true),
                        "F" | "N" => Prop::Bool(false),
                        _ => Prop::String(w.into_bytes()),
                    });
                    self.pos += 1;
                }
                Token::Star => {
                    // `*N { a: values }`
                    self.pos += 1;
                    if let Some(Token::Number(_)) = self.peek() {
                        self.pos += 1;
                    }
                    if let Some(Token::Open) = self.peek() {
                        self.pos += 1;
                        let mut values = Vec::new();
                        let mut array = Vec::new();
                        while let Some(t) = self.peek().cloned() {
                            self.pos += 1;
                            match t {
                                Token::Close => break,
                                Token::Number(w) => array.push(w),
                                Token::Key(_) | Token::Comma | Token::Newline => {}
                                Token::Str(s) => values.push(Prop::String(s)),
                                Token::Word(w) => array.push(w),
                                _ => {}
                            }
                            if array.len() > MAX_ARRAY {
                                return Err(bad("array too large"));
                            }
                        }
                        let floats = array.iter().any(|w| w.contains(['.', 'e', 'E']));
                        let prop = if floats {
                            Prop::F64s(array.iter().filter_map(|w| w.parse().ok()).collect())
                        } else {
                            let ints: Vec<i64> =
                                array.iter().filter_map(|w| w.parse().ok()).collect();
                            if ints.iter().all(|&v| i32::try_from(v).is_ok()) {
                                Prop::I32s(ints.into_iter().map(|v| v as i32).collect())
                            } else {
                                Prop::I64s(ints)
                            }
                        };
                        node.props.push(prop);
                        node.props.extend(values);
                    }
                    break;
                }
                Token::Open => {
                    self.pos += 1;
                    node.children = self.nodes(depth + 1)?;
                    break;
                }
                Token::Close | Token::Key(_) => break,
            }
            after_comma = false;
        }
        Ok(node)
    }
}

fn parse_ascii(bytes: &[u8]) -> Result<Document> {
    let tokens = tokenize(bytes)?;
    let mut parser = Ascii { tokens, pos: 0 };
    let nodes = parser.nodes(0)?;
    if nodes.is_empty() {
        return Err(bad("not an FBX file"));
    }
    let version = nodes
        .iter()
        .find(|n| n.name == "FBXHeaderExtension")
        .and_then(|h| h.value("FBXVersion"))
        .and_then(Prop::as_i64)
        .unwrap_or(0) as u32;
    Ok(Document {
        version,
        binary: false,
        nodes,
    })
}

// ----------------------------------------------------------------- writer

/// Arrays larger than this are zlib-compressed.
const COMPRESS_FROM: usize = 128;

fn write_prop(out: &mut Vec<u8>, prop: &Prop) {
    fn array<T: Copy>(out: &mut Vec<u8>, code: u8, values: &[T], bytes: impl Fn(T) -> Vec<u8>) {
        out.push(code);
        let raw: Vec<u8> = values.iter().flat_map(|&v| bytes(v)).collect();
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        if raw.len() >= COMPRESS_FROM {
            use std::io::Write as _;
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            let _ = encoder.write_all(&raw);
            let packed = encoder.finish().unwrap_or_default();
            out.extend_from_slice(&1u32.to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&packed);
        } else {
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            out.extend_from_slice(&raw);
        }
    }
    match prop {
        Prop::Bool(b) => out.extend_from_slice(&[b'C', *b as u8]),
        Prop::I16(v) => {
            out.push(b'Y');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::I32(v) => {
            out.push(b'I');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::I64(v) => {
            out.push(b'L');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::F32(v) => {
            out.push(b'F');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::F64(v) => {
            out.push(b'D');
            out.extend_from_slice(&v.to_le_bytes());
        }
        Prop::String(s) | Prop::Bytes(s) => {
            out.push(if matches!(prop, Prop::String(_)) {
                b'S'
            } else {
                b'R'
            });
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s);
        }
        Prop::Bools(v) => array(out, b'b', v, |b| vec![b as u8]),
        Prop::I32s(v) => array(out, b'i', v, |x| x.to_le_bytes().to_vec()),
        Prop::I64s(v) => array(out, b'l', v, |x| x.to_le_bytes().to_vec()),
        Prop::F32s(v) => array(out, b'f', v, |x| x.to_le_bytes().to_vec()),
        Prop::F64s(v) => array(out, b'd', v, |x| x.to_le_bytes().to_vec()),
    }
}

/// Nodes that get a null record even without children (as the FBX SDK writes).
const ALWAYS_SENTINEL: [&str; 2] = ["AnimationStack", "AnimationLayer"];

fn write_node(out: &mut Vec<u8>, node: &Node, last: bool) {
    let start = out.len();
    out.extend_from_slice(&[0; 12]);
    out.push(node.name.len().min(255) as u8);
    out.extend_from_slice(&node.name.as_bytes()[..node.name.len().min(255)]);
    let props_start = out.len();
    for p in &node.props {
        write_prop(out, p);
    }
    let props_len = out.len() - props_start;
    if !node.children.is_empty() {
        let n = node.children.len();
        for (k, child) in node.children.iter().enumerate() {
            write_node(out, child, k + 1 == n);
        }
        out.extend_from_slice(&[0; 13]);
    } else if (node.props.is_empty() && !last) || ALWAYS_SENTINEL.contains(&node.name.as_str()) {
        out.extend_from_slice(&[0; 13]);
    }
    let end = out.len() as u32;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&(node.props.len() as u32).to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&(props_len as u32).to_le_bytes());
}

/// Bytes after the top-level nodes that the FBX SDK expects (the same as
/// Blender writes, for a fixed creation time).
const FOOTER_ID: [u8; 16] = [
    0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e,
];
const FOOTER_MAGIC: [u8; 16] = [
    0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b,
];
/// `FileId` matching the fixed creation time.
pub const FILE_ID: [u8; 16] = [
    0x28, 0xb3, 0x2a, 0xeb, 0xb6, 0x24, 0xcc, 0xc2, 0xbf, 0xc8, 0xb0, 0x2a, 0xa9, 0x2b, 0xfc, 0xf1,
];
pub const CREATION_TIME: &str = "1970-01-01 10:00:00:000";

/// Writes a binary FBX file (version 7400: 32-bit offsets).
pub fn write_binary(nodes: &[Node], version: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(BINARY_MAGIC);
    out.extend_from_slice(&[0x1A, 0x00]);
    out.extend_from_slice(&version.to_le_bytes());
    for (k, node) in nodes.iter().enumerate() {
        write_node(&mut out, node, k + 1 == nodes.len());
    }
    out.extend_from_slice(&[0; 13]);
    out.extend_from_slice(&FOOTER_ID);
    out.extend_from_slice(&[0; 4]);
    let pad = match (16 - out.len() % 16) % 16 {
        0 => 16,
        p => p,
    };
    out.extend(std::iter::repeat_n(0u8, pad));
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&[0; 120]);
    out.extend_from_slice(&FOOTER_MAGIC);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_round_trip() {
        let nodes = vec![
            Node::new("Header", vec![Prop::I32(7400)]).with(vec![Node::new(
                "Child",
                vec![Prop::str("a\0\x01b"), Prop::F64(1.5), Prop::Bool(true)],
            )]),
            Node::new(
                "Arrays",
                vec![
                    Prop::F64s((0..1000).map(|i| i as f64 * 0.5).collect()),
                    Prop::I32s(vec![1, -2, 3]),
                    Prop::I64s(vec![1 << 40]),
                    Prop::Bytes(vec![1, 2, 3]),
                ],
            ),
            Node::new("Empty", vec![]),
        ];
        let bytes = write_binary(&nodes, 7400);
        let doc = parse(&bytes).unwrap();
        assert_eq!(doc.version, 7400);
        assert!(doc.binary);
        assert_eq!(doc.nodes, nodes);
    }

    #[test]
    fn ascii_forms() {
        let text = br#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXVersion: 7400
}
Objects:  {
	Geometry: 123, "Geometry::Cube", "Mesh" {
		Vertices: *6 {
			a: 0,0,0,1.5,
			2,3
		}
		PolygonVertexIndex: *3 {
			a: 0,1,-3
		}
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",1,2,3
		}
		Shading: T
	}
}
Connections:  {
	C: "OO",123,0
}
"#;
        let doc = parse(text).unwrap();
        assert_eq!(doc.version, 7400);
        let geometry = doc.get("Objects").unwrap().child("Geometry").unwrap();
        assert_eq!(geometry.props[0], Prop::I32(123));
        assert_eq!(geometry.props[1].as_str().unwrap(), "Geometry::Cube");
        assert_eq!(
            geometry.array_f64("Vertices").unwrap(),
            vec![0.0, 0.0, 0.0, 1.5, 2.0, 3.0]
        );
        assert_eq!(
            geometry.array_i64("PolygonVertexIndex").unwrap(),
            vec![0, 1, -3]
        );
        let p = &geometry.child("Properties70").unwrap().children[0];
        assert_eq!(p.props.len(), 7);
        assert_eq!(p.props[6].as_f64(), Some(3.0));
        assert_eq!(geometry.value("Shading"), Some(&Prop::Bool(true)));
        let c = &doc.get("Connections").unwrap().children[0];
        assert_eq!(c.props.len(), 3);
    }

    #[test]
    fn rejects_garbage() {
        let mut bytes = write_binary(&[Node::new("A", vec![Prop::I32(1)])], 7400);
        assert!(parse(&bytes[..30]).is_err());
        bytes[27] = 0xFF; // end offset beyond the file
        assert!(parse(&bytes).is_err());
    }

    /// A node whose name runs past its end and asks for many properties
    /// (found by fuzzing: the count check wrapped around and 18 GB were
    /// asked for).
    #[test]
    fn a_name_past_the_end_of_its_node_is_an_error() {
        let mut bytes = write_binary(&[Node::new("A", vec![Prop::I32(1)])], 7400);
        // First node at 27: end, property count, property bytes, name length.
        let end = 27 + 13 + 1; // inside the name
        bytes[27..31].copy_from_slice(&(end as u32).to_le_bytes());
        bytes[31..35].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[39] = 5; // name length: past the end, still inside the file
        assert!(parse(&bytes).is_err());
    }
}
