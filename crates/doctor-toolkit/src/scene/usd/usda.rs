//! USD text layers (`#usda 1.0`): parser and writer of the [`Layer`] model.
//!
//! The parser reads prims (`def`/`over`/`class`), their metadata, typed
//! attributes with defaults, `.timeSamples` and `.connect`, relationships,
//! dictionaries and list-edited metadata (`prepend apiSchemas`). Variant
//! sets are skipped. Quaternions are written `(real, i, j, k)` in text and
//! kept as `[i, j, k, real]` in [`Value`].

use std::fmt::Write as _;

use super::{Attribute, Layer, Prim, Property, Specifier, Value, bad};
use crate::Result;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Asset(String),
    Path(String),
    Num(String),
    Punct(char),
}

const MAX_TOKENS: usize = 1 << 28;
const MAX_DEPTH: usize = 128;

fn tokenize(text: &str) -> Result<Vec<Tok>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // Skip the `#usda 1.0` line.
    if text.starts_with("#usda") {
        while i < b.len() && b[i] != b'\n' {
            i += 1;
        }
    }
    while i < b.len() {
        let c = b[i];
        match c {
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'"' | b'\'' => {
                let triple = b[i..].starts_with(&[c, c, c]);
                let (open, close): (usize, &[u8]) = if triple {
                    (3, if c == b'"' { b"\"\"\"" } else { b"'''" })
                } else {
                    (1, std::slice::from_ref(&b[i]))
                };
                let mut j = i + open;
                let mut s = Vec::new();
                loop {
                    if j >= b.len() {
                        return Err(bad("unterminated string"));
                    }
                    if b[j..].starts_with(close) {
                        break;
                    }
                    if b[j] == b'\\' && j + 1 < b.len() {
                        j += 1;
                        s.push(match b[j] {
                            b'n' => b'\n',
                            b't' => b'\t',
                            b'r' => b'\r',
                            b'0' => 0,
                            other => other,
                        });
                    } else {
                        s.push(b[j]);
                    }
                    j += 1;
                }
                out.push(Tok::Str(String::from_utf8_lossy(&s).into_owned()));
                i = j + close.len();
            }
            b'@' => {
                let triple = b[i..].starts_with(b"@@@");
                let close: &[u8] = if triple { b"@@@" } else { b"@" };
                let start = i + close.len();
                let mut j = start;
                while j < b.len() && !b[j..].starts_with(close) {
                    j += 1;
                }
                if j >= b.len() {
                    return Err(bad("unterminated asset path"));
                }
                out.push(Tok::Asset(
                    String::from_utf8_lossy(&b[start..j]).into_owned(),
                ));
                i = j + close.len();
            }
            b'<' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'>' {
                    j += 1;
                }
                if j >= b.len() {
                    return Err(bad("unterminated path"));
                }
                out.push(Tok::Path(
                    String::from_utf8_lossy(&b[start..j]).into_owned(),
                ));
                i = j + 1;
            }
            b'=' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b':' | b';' => {
                out.push(Tok::Punct(c as char));
                i += 1;
            }
            b'0'..=b'9' | b'-' | b'+' | b'.' => {
                let start = i;
                i += 1;
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric()
                        || b[i] == b'.'
                        || ((b[i] == b'-' || b[i] == b'+') && matches!(b[i - 1], b'e' | b'E')))
                {
                    i += 1;
                }
                out.push(Tok::Num(String::from_utf8_lossy(&b[start..i]).into_owned()));
            }
            _ if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                let start = i;
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric()
                        || b[i] == b'_'
                        || b[i] == b':'
                        || b[i] == b'.'
                        || b[i] >= 0x80)
                {
                    i += 1;
                }
                out.push(Tok::Ident(
                    String::from_utf8_lossy(&b[start..i]).into_owned(),
                ));
            }
            other => return Err(bad(format!("unexpected character {:?}", other as char))),
        }
        if out.len() > MAX_TOKENS {
            return Err(bad("file too large"));
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

const LIST_OPS: [&str; 5] = ["prepend", "append", "add", "delete", "reorder"];

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Result<Tok> {
        let t = self
            .toks
            .get(self.pos)
            .cloned()
            .ok_or_else(|| bad("unexpected end of file"))?;
        self.pos += 1;
        Ok(t)
    }

    fn is(&self, c: char) -> bool {
        self.peek() == Some(&Tok::Punct(c))
    }

    fn expect(&mut self, c: char) -> Result<()> {
        match self.next()? {
            Tok::Punct(p) if p == c => Ok(()),
            other => Err(bad(format!("expected `{c}`, found {other:?}"))),
        }
    }

    fn ident(&mut self) -> Result<String> {
        match self.next()? {
            Tok::Ident(s) => Ok(s),
            other => Err(bad(format!("expected a name, found {other:?}"))),
        }
    }

    /// Skips a balanced group starting at the current `(`, `[` or `{`.
    fn skip_group(&mut self) -> Result<()> {
        let mut depth = 0usize;
        loop {
            match self.next()? {
                Tok::Punct('(' | '[' | '{') => depth += 1,
                Tok::Punct(')' | ']' | '}') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }

    /// Any value, untyped.
    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(bad("values nested too deeply"));
        }
        Ok(match self.next()? {
            Tok::Num(n) => number(&n)?,
            Tok::Str(s) => Value::String(s),
            Tok::Asset(a) => Value::Asset(a),
            Tok::Path(p) => Value::Paths(vec![p]),
            Tok::Ident(id) => match id.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "None" => Value::Blocked,
                "inf" => Value::Float(f64::INFINITY),
                "nan" => Value::Float(f64::NAN),
                _ => Value::Token(id),
            },
            Tok::Punct('(') => {
                let mut values = Vec::new();
                while !self.is(')') {
                    if self.is(',') {
                        self.pos += 1;
                        continue;
                    }
                    match self.value(depth + 1)? {
                        Value::Float(f) => values.push(f),
                        Value::Int(i) => values.push(i as f64),
                        Value::Tuple(t) => values.extend(t),
                        Value::Bool(b) => values.push(b as u8 as f64),
                        _ => {}
                    }
                }
                self.expect(')')?;
                Value::Tuple(values)
            }
            Tok::Punct('[') => {
                let mut numbers = Vec::new();
                let mut width = 0;
                let mut strings = Vec::new();
                let mut paths = Vec::new();
                while !self.is(']') {
                    if self.is(',') {
                        self.pos += 1;
                        continue;
                    }
                    match self.value(depth + 1)? {
                        Value::Float(f) => {
                            numbers.push(f);
                            width = 1;
                        }
                        Value::Int(i) => {
                            numbers.push(i as f64);
                            width = 1;
                        }
                        Value::Tuple(t) => {
                            width = t.len();
                            numbers.extend(t);
                        }
                        Value::String(s) | Value::Token(s) | Value::Asset(s) => strings.push(s),
                        Value::Paths(p) => paths.extend(p),
                        _ => {}
                    }
                    if numbers.len() + strings.len() > MAX_TOKENS {
                        return Err(bad("array too large"));
                    }
                }
                self.expect(']')?;
                if !paths.is_empty() {
                    Value::Paths(paths)
                } else if !strings.is_empty() {
                    Value::Strings(strings)
                } else {
                    Value::Array {
                        values: numbers,
                        width: width.max(1),
                    }
                }
            }
            Tok::Punct('{') => {
                // A dictionary: `type name = value` entries.
                let mut entries = Vec::new();
                while !self.is('}') {
                    let t = self.next()?;
                    if !matches!(t, Tok::Ident(_)) {
                        continue;
                    }
                    let mut name = match self.next()? {
                        Tok::Ident(n) | Tok::Str(n) => n,
                        _ => continue,
                    };
                    if self.is('[') {
                        self.pos += 1;
                        self.expect(']')?;
                        name = match self.next()? {
                            Tok::Ident(n) | Tok::Str(n) => n,
                            _ => continue,
                        };
                    }
                    self.expect('=')?;
                    let v = self.value(depth + 1)?;
                    entries.push((name, v));
                }
                self.expect('}')?;
                Value::Dictionary(entries)
            }
            other => return Err(bad(format!("unexpected {other:?} in a value"))),
        })
    }

    /// `( key = value ... )` metadata; bare strings are documentation.
    fn metadata(&mut self) -> Result<Vec<(String, Value)>> {
        let mut out = Vec::new();
        self.expect('(')?;
        while !self.is(')') {
            match self.next()? {
                Tok::Str(s) => out.push(("documentation".into(), Value::String(s))),
                Tok::Ident(mut key) => {
                    if LIST_OPS.contains(&key.as_str())
                        && matches!(self.peek(), Some(Tok::Ident(_)))
                    {
                        key = self.ident()?;
                    }
                    if self.is('=') {
                        self.pos += 1;
                        let v = match self.value(0)? {
                            // Metadata strings are tokens except documentation.
                            Value::String(s) if !matches!(key.as_str(), "doc" | "comment") => {
                                Value::Token(s)
                            }
                            v => v,
                        };
                        out.push((key, v));
                    }
                }
                Tok::Punct(';' | ',') => {}
                Tok::Punct('(' | '[' | '{') => {
                    self.pos -= 1;
                    self.skip_group()?;
                }
                other => return Err(bad(format!("unexpected {other:?} in metadata"))),
            }
        }
        self.expect(')')?;
        Ok(out)
    }

    fn prim(&mut self, specifier: Specifier, depth: usize) -> Result<Prim> {
        if depth > MAX_DEPTH {
            return Err(bad("prims nested too deeply"));
        }
        let mut prim = Prim {
            specifier,
            ..Default::default()
        };
        match self.next()? {
            Tok::Ident(t) => {
                prim.type_name = t;
                match self.next()? {
                    Tok::Str(n) => prim.name = n,
                    other => return Err(bad(format!("expected a prim name, found {other:?}"))),
                }
            }
            Tok::Str(n) => prim.name = n,
            other => return Err(bad(format!("expected a prim name, found {other:?}"))),
        }
        if self.is('(') {
            prim.metadata = self.metadata()?;
        }
        self.expect('{')?;
        while !self.is('}') {
            self.statement(&mut prim, depth)?;
        }
        self.expect('}')?;
        Ok(prim)
    }

    /// One statement in a prim body.
    fn statement(&mut self, prim: &mut Prim, depth: usize) -> Result<()> {
        let first = match self.next()? {
            Tok::Ident(s) => s,
            Tok::Punct(';') => return Ok(()),
            other => return Err(bad(format!("unexpected {other:?} in prim `{}`", prim.name))),
        };
        match first.as_str() {
            "def" => prim.children.push(self.prim(Specifier::Def, depth + 1)?),
            "over" => prim.children.push(self.prim(Specifier::Over, depth + 1)?),
            "class" => prim.children.push(self.prim(Specifier::Class, depth + 1)?),
            "variantSet" => {
                // variantSet "name" = { "a" { ... } ... }
                while !self.is('{') {
                    self.next()?;
                }
                self.skip_group()?;
            }
            "reorder" => {
                self.ident()?;
                self.expect('=')?;
                self.value(0)?;
            }
            _ => {
                let mut word = first;
                if LIST_OPS.contains(&word.as_str()) {
                    word = self.ident()?;
                }
                let mut custom = false;
                let mut uniform = false;
                loop {
                    match word.as_str() {
                        "custom" => custom = true,
                        "uniform" => uniform = true,
                        "varying" | "config" => {}
                        _ => break,
                    }
                    word = self.ident()?;
                }
                if word == "rel" {
                    let name = self.ident()?;
                    let (name, _) = split_suffix(&name);
                    let mut targets = Vec::new();
                    if self.is('=') {
                        self.pos += 1;
                        if let Value::Paths(p) = self.value(0)? {
                            targets = p;
                        }
                    }
                    if self.is('(') {
                        self.metadata()?;
                    }
                    match prim.properties.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, Property::Relationship(t))) => t.extend(targets),
                        _ => prim.relate(&name, targets),
                    }
                    return Ok(());
                }
                // An attribute: type, `[]`, name.
                let mut type_name = word;
                if self.is('[') {
                    self.pos += 1;
                    self.expect(']')?;
                    type_name.push_str("[]");
                }
                let full = self.ident()?;
                let (name, suffix) = split_suffix(&full);
                let index = match prim.properties.iter().position(|(n, _)| *n == name) {
                    Some(i) => i,
                    None => {
                        prim.properties.push((
                            name.clone(),
                            Property::Attribute(Attribute {
                                type_name: type_name.clone(),
                                ..Default::default()
                            }),
                        ));
                        prim.properties.len() - 1
                    }
                };
                let mut attribute = match std::mem::replace(
                    &mut prim.properties[index].1,
                    Property::Relationship(Vec::new()),
                ) {
                    Property::Attribute(a) => a,
                    Property::Relationship(_) => Attribute {
                        type_name: type_name.clone(),
                        ..Default::default()
                    },
                };
                attribute.custom |= custom;
                attribute.uniform |= uniform;
                if self.is('=') {
                    self.pos += 1;
                    match suffix {
                        Some("connect") => {
                            if let Value::Paths(p) = self.value(0)? {
                                attribute.connections.extend(p);
                            }
                        }
                        Some("timeSamples") => {
                            self.expect('{')?;
                            while !self.is('}') {
                                if self.is(',') {
                                    self.pos += 1;
                                    continue;
                                }
                                let time = match self.next()? {
                                    Tok::Num(n) => n
                                        .parse::<f64>()
                                        .map_err(|_| bad(format!("bad time `{n}`")))?,
                                    other => {
                                        return Err(bad(format!(
                                            "expected a time code, found {other:?}"
                                        )));
                                    }
                                };
                                self.expect(':')?;
                                let v = typed(self.value(0)?, &type_name);
                                attribute.samples.push((time, v));
                            }
                            self.expect('}')?;
                            attribute.samples.sort_by(|a, b| a.0.total_cmp(&b.0));
                        }
                        _ => attribute.default = Some(typed(self.value(0)?, &type_name)),
                    }
                }
                if self.is('(') {
                    let meta = self.metadata()?;
                    for (k, v) in meta {
                        attribute.metadata.retain(|m| m.0 != k);
                        attribute.metadata.push((k, v));
                    }
                }
                prim.properties[index].1 = Property::Attribute(attribute);
            }
        }
        Ok(())
    }
}

/// `name.connect` → (`name`, `connect`).
fn split_suffix(full: &str) -> (String, Option<&str>) {
    for suffix in ["connect", "timeSamples", "default", "spline"] {
        if let Some(name) = full.strip_suffix(suffix).and_then(|n| n.strip_suffix('.')) {
            return (name.to_owned(), Some(suffix));
        }
    }
    (full.to_owned(), None)
}

fn number(n: &str) -> Result<Value> {
    if !n.contains(['.', 'e', 'E', 'i', 'n'])
        && let Ok(i) = n.parse::<i64>()
    {
        return Ok(Value::Int(i));
    }
    let lower = n.to_ascii_lowercase();
    let f = match lower.as_str() {
        "-inf" => f64::NEG_INFINITY,
        "+inf" | "inf" => f64::INFINITY,
        "nan" | "-nan" => f64::NAN,
        _ => n
            .parse::<f64>()
            .map_err(|_| bad(format!("bad number `{n}`")))?,
    };
    Ok(Value::Float(f))
}

/// Element width of a USD value type (`float3` → 3), quaternion flag.
pub(crate) fn type_shape(type_name: &str) -> (usize, bool) {
    let base = type_name.trim_end_matches("[]");
    if base.starts_with("quat") {
        return (4, true);
    }
    if base.starts_with("matrix2") {
        return (4, false);
    }
    if base.starts_with("matrix3") {
        return (9, false);
    }
    if base.starts_with("matrix4") || base.starts_with("frame4") {
        return (16, false);
    }
    let width = base
        .bytes()
        .find(|b| b.is_ascii_digit())
        .map_or(1, |d| (d - b'0') as usize);
    (width.clamp(1, 4), false)
}

/// Gives an untyped parsed value the shape of `type_name`.
fn typed(value: Value, type_name: &str) -> Value {
    let base = type_name.trim_end_matches("[]");
    let array = type_name.ends_with("[]");
    let (width, quat) = type_shape(type_name);
    let reorder = |mut v: Vec<f64>| {
        if quat {
            for q in v.chunks_exact_mut(4) {
                q.rotate_left(1);
            }
        }
        v
    };
    match value {
        Value::Blocked => Value::Blocked,
        Value::String(s) | Value::Token(s) if base == "token" => Value::Token(s),
        Value::Token(s) if base == "string" => Value::String(s),
        Value::Strings(v) => Value::Strings(v),
        Value::Array { values, .. } if array => Value::Array {
            values: reorder(values),
            width,
        },
        Value::Tuple(t) if !array => Value::Tuple(reorder(t)),
        Value::Int(i) if matches!(base, "float" | "double" | "half" | "timecode") => {
            Value::Float(i as f64)
        }
        Value::Float(f) if matches!(base, "int" | "uint" | "int64" | "uint64" | "uchar") => {
            Value::Int(f as i64)
        }
        Value::Int(i) if base == "bool" => Value::Bool(i != 0),
        other => other,
    }
}

/// Parses a text layer.
pub fn parse(bytes: &[u8]) -> Result<Layer> {
    let text = String::from_utf8_lossy(bytes);
    let mut p = Parser {
        toks: tokenize(&text)?,
        pos: 0,
    };
    let mut layer = Layer::default();
    if p.is('(') {
        layer.metadata = p.metadata()?;
    }
    while let Some(t) = p.peek().cloned() {
        p.pos += 1;
        match t {
            Tok::Ident(s) if s == "def" => layer.prims.push(p.prim(Specifier::Def, 0)?),
            Tok::Ident(s) if s == "over" => layer.prims.push(p.prim(Specifier::Over, 0)?),
            Tok::Ident(s) if s == "class" => layer.prims.push(p.prim(Specifier::Class, 0)?),
            Tok::Punct(';') => {}
            other => return Err(bad(format!("unexpected {other:?} at the top level"))),
        }
    }
    Ok(layer)
}

// ----------------------------------------------------------------- writer

/// A number: the shortest form that reads back as the same f32 when the
/// value is one, else as the same f64.
fn num(out: &mut String, v: f64) {
    if v.is_nan() {
        out.push_str("nan");
    } else if v.is_infinite() {
        out.push_str(if v > 0.0 { "inf" } else { "-inf" });
    } else if (v as f32) as f64 == v {
        let _ = write!(out, "{}", v as f32);
    } else {
        let _ = write!(out, "{v}");
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn tuple(out: &mut String, values: &[f64], width: usize, quat: bool, matrix: bool) {
    let mut v = values.to_vec();
    if quat {
        for q in v.chunks_exact_mut(4) {
            q.rotate_right(1);
        }
    }
    if matrix && matches!(width, 4 | 9 | 16) {
        let n = (width as f64).sqrt() as usize;
        out.push('(');
        for (r, row) in v.chunks(n).enumerate() {
            if r > 0 {
                out.push_str(", ");
            }
            out.push('(');
            for (k, x) in row.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                num(out, *x);
            }
            out.push(')');
        }
        out.push(')');
        return;
    }
    if width == 1 {
        num(out, v[0]);
        return;
    }
    out.push('(');
    for (k, x) in v.iter().enumerate() {
        if k > 0 {
            out.push_str(", ");
        }
        num(out, *x);
    }
    out.push(')');
}

fn value(out: &mut String, v: &Value, type_name: &str) {
    let (width, quat) = type_shape(type_name);
    let matrix = type_name.starts_with("matrix") || type_name.starts_with("frame");
    match v {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => num(out, *f),
        Value::Token(s) | Value::String(s) => out.push_str(&quote(s)),
        Value::Asset(a) => {
            let _ = write!(out, "@{a}@");
        }
        Value::Tuple(t) => tuple(out, t, width.max(t.len().min(16)), quat, matrix),
        Value::Array { values, width: w } => {
            let w = (*w).max(1);
            out.push('[');
            for (k, element) in values.chunks(w).enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                tuple(out, element, w, quat, matrix);
            }
            out.push(']');
        }
        Value::Strings(s) => {
            out.push('[');
            for (k, x) in s.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                if type_name.starts_with("asset") {
                    let _ = write!(out, "@{x}@");
                } else {
                    out.push_str(&quote(x));
                }
            }
            out.push(']');
        }
        Value::Paths(p) => {
            if p.len() == 1 {
                let _ = write!(out, "<{}>", p[0]);
            } else {
                out.push('[');
                for (k, x) in p.iter().enumerate() {
                    if k > 0 {
                        out.push_str(", ");
                    }
                    let _ = write!(out, "<{x}>");
                }
                out.push(']');
            }
        }
        Value::Dictionary(entries) => {
            out.push_str("{ ");
            for (k, v) in entries {
                let kind = match v {
                    Value::Bool(_) => "bool",
                    Value::Int(_) => "int",
                    Value::Float(_) => "double",
                    Value::Dictionary(_) => "dictionary",
                    _ => "string",
                };
                let _ = write!(out, "{kind} {k} = ");
                value(out, v, kind);
                out.push(' ');
            }
            out.push('}');
        }
        Value::Blocked => out.push_str("None"),
    }
}

fn metadata(out: &mut String, entries: &[(String, Value)], indent: &str) {
    if entries.is_empty() {
        return;
    }
    out.push_str(" (\n");
    for (k, v) in entries {
        let list = if k == "apiSchemas" { "prepend " } else { "" };
        let _ = write!(out, "{indent}    {list}{k} = ");
        let type_name = match v {
            Value::Strings(_) => "token[]",
            _ => "",
        };
        value(out, v, type_name);
        out.push('\n');
    }
    let _ = write!(out, "{indent})");
}

fn prim(out: &mut String, p: &Prim, depth: usize) {
    let indent = "    ".repeat(depth);
    let specifier = match p.specifier {
        Specifier::Def => "def",
        Specifier::Over => "over",
        Specifier::Class => "class",
    };
    let _ = write!(out, "{indent}{specifier} ");
    if !p.type_name.is_empty() {
        let _ = write!(out, "{} ", p.type_name);
    }
    out.push_str(&quote(&p.name));
    metadata(out, &p.metadata, &indent);
    let _ = writeln!(out, "\n{indent}{{");
    for (name, property) in &p.properties {
        let _ = write!(out, "{indent}    ");
        match property {
            Property::Relationship(targets) => {
                out.push_str("rel ");
                out.push_str(name);
                if !targets.is_empty() {
                    out.push_str(" = ");
                    value(out, &Value::Paths(targets.clone()), "");
                }
                out.push('\n');
            }
            Property::Attribute(a) => {
                if a.custom {
                    out.push_str("custom ");
                }
                if a.uniform {
                    out.push_str("uniform ");
                }
                let declaration = format!("{} {name}", a.type_name);
                out.push_str(&declaration);
                if let Some(v) = &a.default {
                    out.push_str(" = ");
                    value(out, v, &a.type_name);
                }
                if !a.metadata.is_empty() {
                    metadata(out, &a.metadata, &format!("{indent}    "));
                }
                out.push('\n');
                if !a.samples.is_empty() {
                    let _ = writeln!(out, "{indent}    {declaration}.timeSamples = {{");
                    for (t, v) in &a.samples {
                        let _ = write!(out, "{indent}        ");
                        num(out, *t);
                        out.push_str(": ");
                        value(out, v, &a.type_name);
                        out.push_str(",\n");
                    }
                    let _ = writeln!(out, "{indent}    }}");
                }
                if !a.connections.is_empty() {
                    let _ = write!(out, "{indent}    {declaration}.connect = ");
                    value(out, &Value::Paths(a.connections.clone()), "");
                    out.push('\n');
                }
            }
        }
    }
    for child in &p.children {
        out.push('\n');
        self::prim(out, child, depth + 1);
    }
    let _ = writeln!(out, "{indent}}}");
}

/// Writes a text layer.
pub fn write(layer: &Layer) -> String {
    let mut out = String::from("#usda 1.0\n");
    if !layer.metadata.is_empty() {
        metadata(&mut out, &layer.metadata, "");
    }
    out.push('\n');
    for p in &layer.prims {
        out.push('\n');
        prim(&mut out, p, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blender_style_text() {
        let text = br#"#usda 1.0
(
    defaultPrim = "root"
    doc = """Blender
v5"""
    metersPerUnit = 1
    upAxis = "Z"
)

def Xform "root" (
    prepend apiSchemas = ["ColorSpaceAPI"]
    customData = {
        dictionary Blender = {
            bool generated = 1
        }
    }
)
{
    float3 xformOp:rotateXYZ.timeSamples = {
        0: (0, -0, 90),
    }
    uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]

    def Mesh "m" (
        active = true
    )
    {
        int[] faceVertexCounts = [3]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1e-3, 0)]
        texCoord2f[] primvars:st = [(0, 1), (1, 1), (0, 0)] (
            interpolation = "faceVarying"
        )
        int[] primvars:st:indices.timeSamples = {
            0: [0, 1, 2],
        }
        quatf[] rotations = [(1, 0, 0, 0)]
        matrix4d m = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (5, 6, 7, 1) )
        rel material:binding = </root/_materials/a>
        color3f inputs:diffuseColor.connect = </root/x.outputs:rgb>
        asset inputs:file = @./textures/a b.png@
        string inputs:varname = "st"
    }
}
"#;
        let layer = parse(text).unwrap();
        assert_eq!(layer.meta("upAxis"), Some(&Value::Token("Z".into())));
        assert_eq!(layer.meta("documentation"), None);
        let root = &layer.prims[0];
        assert_eq!(root.type_name, "Xform");
        assert_eq!(
            root.meta("apiSchemas"),
            Some(&Value::Strings(vec!["ColorSpaceAPI".into()]))
        );
        let rot = root.attribute("xformOp:rotateXYZ").unwrap();
        assert_eq!(rot.value(), Some(&Value::Tuple(vec![0.0, -0.0, 90.0])));
        let mesh = layer.prim("/root/m").unwrap();
        let points = mesh.attribute("points").unwrap();
        assert_eq!(points.type_name, "point3f[]");
        assert_eq!(
            points.value(),
            Some(&Value::Array {
                values: vec![0., 0., 0., 1., 0., 0., 0., 1e-3, 0.],
                width: 3
            })
        );
        let st = mesh.attribute("primvars:st").unwrap();
        assert_eq!(
            st.meta("interpolation"),
            Some(&Value::Token("faceVarying".into()))
        );
        assert_eq!(
            mesh.attribute("primvars:st:indices").unwrap().samples.len(),
            1
        );
        // Quaternions are kept as [i, j, k, real].
        assert_eq!(
            mesh.attribute("rotations").unwrap().value(),
            Some(&Value::Array {
                values: vec![0., 0., 0., 1.],
                width: 4
            })
        );
        let m = mesh
            .attribute("m")
            .unwrap()
            .value()
            .unwrap()
            .numbers()
            .unwrap();
        assert_eq!(m[12..15], [5.0, 6.0, 7.0]);
        assert_eq!(
            mesh.relationship("material:binding").unwrap(),
            ["/root/_materials/a"]
        );
        assert_eq!(
            mesh.attribute("inputs:diffuseColor").unwrap().connections,
            ["/root/x.outputs:rgb"]
        );
        assert_eq!(
            mesh.attribute("inputs:file").unwrap().value(),
            Some(&Value::Asset("./textures/a b.png".into()))
        );
    }

    #[test]
    fn writes_what_it_reads() {
        let mut prim = Prim::new("a", "Mesh");
        prim.metadata.push((
            "apiSchemas".into(),
            Value::Strings(vec!["SkelBindingAPI".into()]),
        ));
        let mut a = Attribute::new(
            "quatf[]",
            Value::Array {
                values: vec![0.1, 0.2, 0.3, 0.9],
                width: 4,
            },
        );
        a.samples.push((
            2.0,
            Value::Array {
                values: vec![0.0, 0.0, 0.0, 1.0],
                width: 4,
            },
        ));
        a.metadata
            .push(("interpolation".into(), Value::Token("vertex".into())));
        prim.set("rotations", a);
        let mut c = Attribute::new("float3", Value::Tuple(vec![0.5, 1.0, 1e-7]));
        c.connections.push("/a/b.outputs:rgb".into());
        prim.set("inputs:c", c);
        prim.set(
            "m",
            Attribute::new(
                "matrix4d",
                Value::Tuple((0..16).map(|i| i as f64).collect()),
            ),
        );
        prim.relate("skel:skeleton", vec!["/a/s".into()]);
        let mut child = Prim::new("s", "Skeleton");
        child.set(
            "joints",
            Attribute {
                uniform: true,
                ..Attribute::new("token[]", Value::Strings(vec!["r".into(), "r/c".into()]))
            },
        );
        prim.children.push(child);
        let layer = Layer {
            metadata: vec![("upAxis".into(), Value::Token("Y".into()))],
            prims: vec![prim],
        };
        let text = write(&layer);
        let back = parse(text.as_bytes()).unwrap();
        assert_eq!(
            back.prims[0].properties, layer.prims[0].properties,
            "{text}"
        );
        assert_eq!(back.prims[0].children, layer.prims[0].children);
        assert_eq!(back.prims[0].metadata, layer.prims[0].metadata);
    }
}
