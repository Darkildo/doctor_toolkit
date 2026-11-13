//! Global search over a [`GameIndex`].
//!
//! A query is a line of words:
//!
//! | Word | Matches |
//! |---|---|
//! | `wheel`, `"rear wheel"` | names containing the text, any case |
//! | `car_*_lod?` | names matching the glob (`*`, `?`), whole name |
//! | `0x9cf57f36`, `9cf57f36` | ids, hashes in the data, reference targets, string entries |
//! | `gf2/frontend:tg4h:…` | the resource with this key |
//! | `type:mesh,material` | only these types |
//! | `stream:mp_mall` | only streams whose name contains the text |
//! | `kind:object` | only names of this kind ([`NameKind::label`]) |
//! | `text:Don Corleone` | strings of string tables (the rest of the line) |
//!
//! Plain words are joined with single spaces into one name pattern. A query
//! with only `type:` / `stream:` lists every resource that passes the filters.

use rayon::prelude::*;

use crate::resource::names::type_tags;
use crate::workspace::ResourceKey;
use crate::{Error, Result};

use super::index::{Anchor, GameIndex, HashKind, NameKind, RecordId};

/// A parsed search query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Name pattern in lower case; a glob when it contains `*` or `?`.
    pub pattern: Option<String>,
    pub hash: Option<u32>,
    pub key: Option<ResourceKey>,
    /// Text of string tables, in lower case.
    pub text: Option<String>,
    pub types: Vec<u32>,
    /// Parts of stream names, in lower case.
    pub streams: Vec<String>,
    pub kinds: Vec<NameKind>,
}

/// Why a resource matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// The query was a key, or only filters.
    Resource,
    /// One of the four ids.
    Id,
    Name {
        kind: NameKind,
        text: String,
    },
    /// A hash stored next to a name.
    NameHash(String),
    /// A hash found in the data.
    Hash(HashKind),
    /// A reference to the hash.
    Ref(String),
    /// A material name hash.
    Material,
    Text {
        hash: u32,
        language: String,
        text: String,
    },
}

impl Hit {
    pub fn describe(&self) -> String {
        match self {
            Self::Resource => "resource".into(),
            Self::Id => "id".into(),
            Self::Name { kind, text } => format!("{} `{text}`", kind.label()),
            Self::NameHash(name) => format!("name hash of `{name}`"),
            Self::Hash(kind) => format!("{} hash", kind.label()),
            Self::Ref(label) => format!("reference {label}"),
            Self::Material => "material name hash".into(),
            Self::Text {
                hash,
                language,
                text,
            } => format!("string {hash:08x} [{language}] {text:?}"),
        }
    }
}

/// A matching resource with the reasons it matched (at most a few).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub record: RecordId,
    pub hits: Vec<Hit>,
}

/// Hits kept per resource.
const MAX_HITS: usize = 4;

impl Query {
    /// Parses a query line, see the module documentation.
    pub fn parse(line: &str) -> Result<Self> {
        let mut query = Self::default();
        let mut words = Vec::new();
        let mut rest = line.trim();
        while !rest.is_empty() {
            if let Some(text) = rest.strip_prefix("text:") {
                let text = text.trim().trim_matches('"');
                if text.is_empty() {
                    return Err(bad(line, "`text:` needs some text"));
                }
                query.text = Some(text.to_lowercase());
                break;
            }
            let (word, quoted, next) = if let Some(inner) = rest.strip_prefix('"') {
                match inner.find('"') {
                    Some(end) => (&inner[..end], true, &inner[end + 1..]),
                    None => (inner, true, ""),
                }
            } else {
                match rest.find(char::is_whitespace) {
                    Some(end) => (&rest[..end], false, &rest[end..]),
                    None => (rest, false, ""),
                }
            };
            rest = next.trim_start();
            if quoted {
                words.push(word.to_owned());
                continue;
            }
            if let Some(list) = word.strip_prefix("type:") {
                for tag in list.split(',').filter(|t| !t.is_empty()) {
                    let id = type_tags()
                        .resolve(tag)
                        .ok_or_else(|| bad(line, &format!("unknown type `{tag}`")))?;
                    query.types.push(id);
                }
            } else if let Some(stream) = word.strip_prefix("stream:") {
                query
                    .streams
                    .push(stream.replace('\\', "/").to_ascii_lowercase());
            } else if let Some(kind) = word.strip_prefix("kind:") {
                let kind = NameKind::ALL
                    .into_iter()
                    .find(|k| k.label() == kind)
                    .ok_or_else(|| bad(line, &format!("unknown name kind `{kind}`")))?;
                query.kinds.push(kind);
            } else if let Some(hash) = word
                .strip_prefix("hash:")
                .or_else(|| word.strip_prefix("id:"))
            {
                query.hash = Some(parse_hash(hash).ok_or_else(|| bad(line, "bad hash"))?);
            } else if let Some(key) = word.strip_prefix("key:") {
                query.key = Some(key.parse()?);
            } else if word.matches(':').count() >= 2
                && let Ok(key) = word.parse::<ResourceKey>()
            {
                query.key = Some(key);
            } else if let Some(hash) = parse_hash(word).filter(|_| looks_like_hash(word)) {
                query.hash = Some(hash);
            } else {
                words.push(word.to_owned());
            }
        }
        if !words.is_empty() {
            query.pattern = Some(words.join(" ").to_lowercase());
        }
        Ok(query)
    }

    /// Whether the query only filters by type or stream.
    pub fn is_filter_only(&self) -> bool {
        self.pattern.is_none() && self.hash.is_none() && self.key.is_none() && self.text.is_none()
    }

    /// Whether the query asks for anything at all.
    pub fn is_empty(&self) -> bool {
        self.is_filter_only() && self.types.is_empty() && self.streams.is_empty()
    }
}

fn bad(line: &str, why: &str) -> Error {
    Error::InvalidArgument(format!("bad search `{line}`: {why}"))
}

/// `0x` followed by hexadecimal digits, or exactly eight hexadecimal digits
/// with at least one digit 0-9.
fn looks_like_hash(word: &str) -> bool {
    word.starts_with("0x")
        || (word.len() == 8
            && word.chars().all(|c| c.is_ascii_hexdigit())
            && word.chars().any(|c| c.is_ascii_digit()))
}

fn parse_hash(word: &str) -> Option<u32> {
    let digits = word.strip_prefix("0x").unwrap_or(word);
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Case-insensitive glob match of a whole string; `pattern` is lower case.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Whether `text` contains `needle` (lower case), ignoring ASCII case.
pub fn contains_ignore_case(text: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if text.is_ascii() && needle.is_ascii() {
        let (t, n) = (text.as_bytes(), needle.as_bytes());
        t.windows(n.len())
            .any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b))
    } else {
        text.to_lowercase().contains(needle)
    }
}

/// Runs a query. Returns at most `limit` results (0 = all) in index order and
/// the total number of matching resources.
pub fn search(index: &GameIndex, query: &Query, limit: usize) -> (Vec<SearchResult>, usize) {
    if let Some(key) = &query.key {
        let found: Vec<_> = index
            .find_key(key)
            .map(|record| SearchResult {
                record,
                hits: vec![Hit::Resource],
            })
            .into_iter()
            .collect();
        let total = found.len();
        return (found, total);
    }
    let glob = query
        .pattern
        .as_ref()
        .is_some_and(|p| p.contains(['*', '?']));
    let stream_ok: Vec<bool> = (0..index.stream_count())
        .map(|s| {
            query.streams.is_empty()
                || query
                    .streams
                    .iter()
                    .any(|part| index.stream_name(s).contains(part.as_str()))
        })
        .collect();
    let mut results: Vec<SearchResult> = (0..index.stream_count())
        .into_par_iter()
        .filter(|&s| stream_ok[s])
        .flat_map_iter(|s| index.stream_records(s))
        .filter_map(|id| {
            let record = index.record(id);
            if !query.types.is_empty() && !query.types.contains(&record.header.type_id) {
                return None;
            }
            if query.is_filter_only() {
                return Some(SearchResult {
                    record: id,
                    hits: vec![Hit::Resource],
                });
            }
            let mut hits = Vec::new();
            if let Some(pattern) = &query.pattern {
                for (kind, name) in &record.names {
                    if !query.kinds.is_empty() && !query.kinds.contains(kind) {
                        continue;
                    }
                    let matched = if glob {
                        glob_match(pattern, name)
                    } else {
                        contains_ignore_case(name, pattern)
                    };
                    if matched {
                        hits.push(Hit::Name {
                            kind: *kind,
                            text: name.clone(),
                        });
                        if hits.len() == MAX_HITS {
                            break;
                        }
                    }
                }
                if hits.is_empty() {
                    return None;
                }
            }
            if let Some(hash) = query.hash {
                let before = hits.len();
                if record.header.ids.contains(&hash) {
                    hits.push(Hit::Id);
                }
                for (h, name) in &record.pairs {
                    if *h == hash {
                        hits.push(Hit::NameHash(name.clone()));
                    }
                }
                for (kind, h) in &record.hashes {
                    if *h == hash {
                        hits.push(Hit::Hash(*kind));
                        break;
                    }
                }
                for r in &record.refs {
                    if r.target.hash() == Some(hash) {
                        hits.push(Hit::Ref(r.label.clone()));
                        break;
                    }
                }
                if record.anchors.contains(&Anchor::Material(hash)) {
                    hits.push(Hit::Material);
                }
                for t in record
                    .texts
                    .iter()
                    .filter(|t| t.hash == hash)
                    .take(MAX_HITS)
                {
                    hits.push(text_hit(record, t));
                }
                if hits.len() == before {
                    return None;
                }
            }
            if let Some(text) = &query.text {
                let before = hits.len();
                let found = record
                    .texts
                    .iter()
                    .filter(|t| contains_ignore_case(&t.text, text))
                    .take(MAX_HITS);
                hits.extend(found.map(|t| text_hit(record, t)));
                if hits.len() == before {
                    return None;
                }
            }
            hits.truncate(MAX_HITS);
            Some(SearchResult { record: id, hits })
        })
        .collect();
    let total = results.len();
    if limit != 0 {
        results.truncate(limit);
    }
    (results, total)
}

fn text_hit(record: &super::index::Record, t: &super::index::TextEntry) -> Hit {
    Hit::Text {
        hash: t.hash,
        language: record
            .languages
            .get(t.language as usize)
            .cloned()
            .unwrap_or_default(),
        text: t.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_filters_hashes_keys_and_words() {
        let q = Query::parse("type:mesh,material stream:MP_Mall rear wheel").unwrap();
        assert_eq!(q.types, [0x0196_1BED, 0x5949_0267]);
        assert_eq!(q.streams, ["mp_mall"]);
        assert_eq!(q.pattern.as_deref(), Some("rear wheel"));

        let q = Query::parse("0x9CF57F36").unwrap();
        assert_eq!(q.hash, Some(0x9CF5_7F36));
        assert_eq!(Query::parse("9cf57f36").unwrap().hash, Some(0x9CF5_7F36));
        // Eight letters a-f are a name, not a hash.
        assert_eq!(
            Query::parse("deadbeef").unwrap().pattern.as_deref(),
            Some("deadbeef")
        );
        assert_eq!(Query::parse("\"9cf57f36\"").unwrap().hash, None);

        let q = Query::parse("gf2/text/dd7d7050:lh2:9e583c54").unwrap();
        assert_eq!(q.key.unwrap().to_string(), "gf2/text/dd7d7050:lh2:9e583c54");

        let q = Query::parse("type:lh2 text: Don  Corleone ").unwrap();
        assert_eq!(q.text.as_deref(), Some("don  corleone"));
        assert!(Query::parse("type:nosuchtype").is_err());
        assert!(Query::parse("kind:object").unwrap().is_filter_only());
        assert!(Query::parse("").unwrap().is_empty());
    }

    #[test]
    fn globs_and_substrings() {
        assert!(glob_match("car_*_lod?", "Car_Sedan_LOD1"));
        assert!(!glob_match("car_*_lod?", "car_sedan_lod12"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*wheel*", "rear_wheel_l"));
        assert!(contains_ignore_case("Rear_Wheel", "wheel"));
        assert!(!contains_ignore_case("Rear", "wheel"));
        assert!(contains_ignore_case("Дон Корлеоне", "корлеоне"));
    }
}
