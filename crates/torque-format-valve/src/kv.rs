//! KeyValues text: the nested `"key" "value"` and `"key" { ... }` lists of materials (`.vmt`), a map's entities, a game's
//! `gameinfo.txt` and most of the Source engine's text files.
//!
//! Tokens are quoted strings (a backslash is an ordinary character: the engine reads materials and maps without escape
//! sequences, and their paths use backslashes), bare words (up to white space, a quote or a brace), `{` and `}`;
//! `//` starts a comment to the end of the line; a `[$CONDITION]` after a pair is skipped (every condition is taken as true);
//! `#include` and `#base` lines are skipped. A brace with no key before it (a map's entity list) reads as a block named `""`.
//! The parser caps its depth and its pairs, and never panics.

use crate::read::{Error, Result, invalid};

/// The deepest nesting read.
pub const MAX_DEPTH: usize = 64;
/// The most pairs one text may hold.
pub const MAX_PAIRS: usize = 1 << 21;

/// A value: a string or a block of pairs.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Block(Vec<Pair>),
}

/// A key and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub key: String,
    pub value: Value,
}

impl Pair {
    /// The value's string, if it is one.
    pub fn str(&self) -> Option<&str> {
        match &self.value {
            Value::Str(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    /// The value's pairs, if it is a block.
    pub fn block(&self) -> Option<&[Pair]> {
        match &self.value {
            Value::Block(b) => Some(b),
            Value::Str(_) => None,
        }
    }
}

/// The first pair of `pairs` whose key is `key` (any case).
pub fn find<'a>(pairs: &'a [Pair], key: &str) -> Option<&'a Pair> {
    pairs.iter().find(|p| p.key.eq_ignore_ascii_case(key))
}

/// The string value of the first pair whose key is `key` (any case).
pub fn find_str<'a>(pairs: &'a [Pair], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .filter(|p| p.key.eq_ignore_ascii_case(key))
        .find_map(Pair::str)
}

/// The block of the first pair whose key is `key` (any case).
pub fn find_block<'a>(pairs: &'a [Pair], key: &str) -> Option<&'a [Pair]> {
    pairs
        .iter()
        .filter(|p| p.key.eq_ignore_ascii_case(key))
        .find_map(Pair::block)
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
    Cond,
}

struct Lexer<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn next(&mut self) -> Result<Option<Tok>> {
        loop {
            while self.pos < self.b.len() && self.b[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            let Some(&c) = self.b.get(self.pos) else {
                return Ok(None);
            };
            if c == b'/' && self.b.get(self.pos + 1) == Some(&b'/') {
                while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            if c == b'#' {
                // `#include "file"` and `#base "file"`: the line is skipped.
                while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            return match c {
                b'{' => {
                    self.pos += 1;
                    Ok(Some(Tok::Open))
                }
                b'}' => {
                    self.pos += 1;
                    Ok(Some(Tok::Close))
                }
                b'[' => {
                    while self.pos < self.b.len() && self.b[self.pos] != b']' {
                        self.pos += 1;
                    }
                    self.pos += 1;
                    Ok(Some(Tok::Cond))
                }
                b'"' => {
                    self.pos += 1;
                    let mut s = Vec::new();
                    // An unterminated string ends at the end of the text, as the engine reads it.
                    while let Some(&c) = self.b.get(self.pos) {
                        self.pos += 1;
                        if c == b'"' {
                            break;
                        }
                        s.push(c);
                    }
                    Ok(Some(Tok::Str(String::from_utf8_lossy(&s).into_owned())))
                }
                _ => {
                    let start = self.pos;
                    while self.pos < self.b.len() {
                        let c = self.b[self.pos];
                        if c.is_ascii_whitespace() || matches!(c, b'"' | b'{' | b'}') {
                            break;
                        }
                        if c == b'/' && self.b.get(self.pos + 1) == Some(&b'/') {
                            break;
                        }
                        self.pos += 1;
                    }
                    Ok(Some(Tok::Str(
                        String::from_utf8_lossy(&self.b[start..self.pos]).into_owned(),
                    )))
                }
            };
        }
    }
}

/// Reads a KeyValues text (a leading UTF-8 byte-order mark is skipped).
pub fn parse(text: &[u8]) -> Result<Vec<Pair>> {
    let text = text.strip_prefix(b"\xef\xbb\xbf").unwrap_or(text);
    let mut lx = Lexer { b: text, pos: 0 };
    let mut pairs = 0usize;
    // An explicit stack instead of recursion: the depth is capped, and no hostile nesting reaches the native stack.
    let mut stack: Vec<(String, Vec<Pair>)> = vec![(String::new(), Vec::new())];
    let mut key: Option<String> = None;
    while let Some(t) = lx.next()? {
        match t {
            Tok::Cond => {}
            Tok::Str(s) => match key.take() {
                None => key = Some(s),
                Some(k) => {
                    pairs += 1;
                    if pairs > MAX_PAIRS {
                        return Err(Error::TooLarge {
                            what: "the KeyValues pairs",
                            asked: pairs as u64,
                            cap: MAX_PAIRS as u64,
                        });
                    }
                    if let Some(top) = stack.last_mut() {
                        top.1.push(Pair {
                            key: k,
                            value: Value::Str(s),
                        });
                    }
                }
            },
            Tok::Open => {
                if stack.len() > MAX_DEPTH {
                    return invalid("KeyValues nested too deep");
                }
                stack.push((key.take().unwrap_or_default(), Vec::new()));
            }
            Tok::Close => {
                // A key with no value before a brace is dropped.
                key = None;
                if stack.len() == 1 {
                    // A stray closing brace at the top: the rest is ignored, as the engine stops there.
                    break;
                }
                if let Some((k, items)) = stack.pop() {
                    pairs += 1;
                    if let Some(top) = stack.last_mut() {
                        top.1.push(Pair {
                            key: k,
                            value: Value::Block(items),
                        });
                    }
                }
            }
        }
    }
    // Blocks left open at the end close there.
    while stack.len() > 1 {
        if let Some((k, items)) = stack.pop()
            && let Some(top) = stack.last_mut()
        {
            top.1.push(Pair {
                key: k,
                value: Value::Block(items),
            });
        }
    }
    Ok(stack.pop().map(|s| s.1).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_material() {
        let t = br#"
            // a comment
            "LightmappedGeneric"
            {
                "$basetexture" "concrete/wall01"   // trailing
                $surfaceprop concrete
                "$alpha" "0.5" [$X360]
                "Proxies" { "AnimatedTexture" { "animatedtextureframerate" "10" } }
            }
        "#;
        let kv = parse(t).unwrap();
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].key, "LightmappedGeneric");
        let b = kv[0].block().unwrap();
        assert_eq!(find_str(b, "$BaseTexture"), Some("concrete/wall01"));
        assert_eq!(find_str(b, "$surfaceprop"), Some("concrete"));
        assert_eq!(find_str(b, "$alpha"), Some("0.5"));
        assert!(find_block(b, "proxies").is_some());
    }

    #[test]
    fn reads_entities() {
        let t = b"{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"light\"\n\"origin\" \"1 2 3\"\n}\n";
        let kv = parse(t).unwrap();
        assert_eq!(kv.len(), 2);
        assert_eq!(find_str(kv[1].block().unwrap(), "origin"), Some("1 2 3"));
    }

    #[test]
    fn survives_hostile_text() {
        assert!(parse(&[b'{'; 100]).is_err());
        assert!(parse(b"\"unterminated").is_ok());
        assert!(parse(b"}}}{").is_ok());
        assert!(parse(b"a \\").is_ok());
    }
}
