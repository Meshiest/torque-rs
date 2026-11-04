//! The scanner: this crate's own longest-match scanner over Torque3D's token rules (`CMDscan.l`, part of the MIT-licensed
//! Torque3D tree, Copyright (c) 2012 GarageGames, LLC). The rules, in that file's order, which is their priority (an
//! equally long match goes to the earlier rule):
//!
//! | pattern | token |
//! |---|---|
//! | `[ \t\v\f]+` | skipped |
//! | `//[^\n\r]*` | skipped |
//! | `\r` | skipped |
//! | `\n` | skipped; the line counter goes up |
//! | `"(\\[^\n]\|[^\\"\n\r])*"`, the same with `'` | string, tagged string (escapes decoded by `collapseEscape`) |
//! | `==` `!=` `>=` `<=` `&&` `\|\|` `::` `--` `++` `$=` `!$=` `<<` `>>` `+=` `-=` `*=` `/=` `%=` `&=` `^=` `\|=` `<<=` `>>=` | operators |
//! | `->` `-->` | the internal-name operators |
//! | `NL` `TAB` `SPC` `@` | `@` with the byte 10, 9, 32 or none |
//! | `/*` | skips to the next `*/` (counting lines); the end of the input first is [`Lexed::UnterminatedComment`] |
//! | one of `?[]()+-*/<>\|.!:;{},&%^~=` | itself |
//! | `in or break return else assert while do if foreach$ foreach for continue function new singleton datablock case switch$ switch default package`, `true`, `false` | keywords (case-sensitive; `namespace` is not a keyword); `true`/`false` are the integers 1 and 0 |
//! | `[$%][A-Za-z_]([A-Za-z0-9_:]*[A-Za-z0-9_])?` | variable |
//! | `[A-Za-z_][A-Za-z0-9_]*` | identifier |
//! | `0[xX][0-9a-fA-F]+`, `[0-9]+` | integer (`sscanf("%x")`, `atoi`) |
//! | `[0-9]*\.[0-9]+([eE][+-]?[0-9]+)?\|[0-9]+[eE][+-]?[0-9]+` | float (`atof`, rounded to a single-precision number) |
//! | `[$%][0-9]+[A-Za-z_]([A-Za-z0-9_:]*[A-Za-z0-9_])?`, any byte but `\n` | illegal token |
//!
//! Torque3D's doc comments (`///` lines, its `DOCBLOCK` token) are not scanned: they are plain `//` comments here.
//! A token may be any length.
//!
//! **The classic language.** [`Language::Tge14`] scans TGE 1.4's `CMDscan.l` (`engine/console/`, which carries no
//! notice of its own; the tree is Copyright (C) GarageGames.com, Inc.), which has the same rules without Torque3D's
//! additions: `in`, `assert`, `foreach`, `foreach$` and `singleton` are names, `->` and `-->` are `-`, `-` and `>`;
//! `namespace` is a keyword that no rule of the grammar takes (an illegal token); a float needs digits before its point (`.5` is `.` and `5`).
//!
//! **The read position.** The syntax error report shows the text around the scanner's read position
//! ([`Lexer::scan_index`]). The input is read as `CMDscan.l`'s `YY_INPUT` reads it from `CMDgetc`: a line at a time, up
//! to and including its newline, whenever a scan needs a byte past what was read; so the read position is the end of
//! the line of the furthest byte a scan has looked at. A scan looks at bytes while some rule can still match a longer
//! text, then at the one byte that stops it (none when no rule can go on at all). A NUL ends the input; every other
//! byte, 0xFF included, is text.
//!
//! Identifiers and variables are interned ignoring case: every later spelling of a name is the first one's.
//! A running engine shares one `StringTable` between all files, so there the first spelling can come from an
//! earlier file; this compiler keeps one table per file. The DSO's bytes can differ, never the behaviour (names
//! are looked up ignoring case).

use std::collections::HashMap;

use torque_ts_text::{atof, atoi, collapse_escape_checked, to_f32, to_f64};

use crate::ast::Name;

/// Which token rules the scanner has (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    /// TGE 1.4's.
    Tge14,
    /// Torque3D's.
    #[default]
    Torque3d,
}

/// The keywords Torque3D added to TGE 1.4's.
const TORQUE3D_KEYWORDS: [&[u8]; 5] = [b"in", b"assert", b"foreach$", b"foreach", b"singleton"];

/// Terminal numbers of the grammar (`parser.rs`): compact, not yacc's numbers.
pub mod tok {
    pub const END: u8 = 0;
    pub const FUNCTION: u8 = 1;
    pub const NEW: u8 = 2;
    pub const BREAK: u8 = 3;
    pub const ELSE: u8 = 4;
    pub const CONTINUE: u8 = 5;
    pub const IF: u8 = 6;
    pub const RETURN: u8 = 7;
    pub const WHILE: u8 = 8;
    pub const DEFAULT: u8 = 9;
    pub const FOR: u8 = 10;
    pub const DATABLOCK: u8 = 11;
    pub const SWITCH: u8 = 12;
    pub const CASE: u8 = 13;
    pub const SWITCHSTR: u8 = 14;
    pub const CASEOR: u8 = 15;
    pub const PACKAGE: u8 = 16;
    pub const ILLEGAL: u8 = 17;
    pub const INTCONST: u8 = 18;
    pub const VAR: u8 = 19;
    pub const IDENT: u8 = 20;
    pub const STRATOM: u8 = 21;
    pub const TAGATOM: u8 = 22;
    pub const FLTCONST: u8 = 23;
    pub const MINUSMINUS: u8 = 24;
    pub const PLUSPLUS: u8 = 25;
    pub const SHL: u8 = 26;
    pub const SHR: u8 = 27;
    pub const PLASN: u8 = 28;
    pub const MIASN: u8 = 29;
    pub const MLASN: u8 = 30;
    pub const DVASN: u8 = 31;
    pub const MODASN: u8 = 32;
    pub const ANDASN: u8 = 33;
    pub const XORASN: u8 = 34;
    pub const ORASN: u8 = 35;
    pub const SLASN: u8 = 36;
    pub const SRASN: u8 = 37;
    pub const EQ: u8 = 38;
    pub const NE: u8 = 39;
    pub const GE: u8 = 40;
    pub const LE: u8 = 41;
    pub const ANDAND: u8 = 42;
    pub const OROR: u8 = 43;
    pub const STREQ: u8 = 44;
    pub const COLONCOLON: u8 = 45;
    pub const STRNE: u8 = 46;
    pub const DO: u8 = 47;
    pub const FOREACH: u8 = 48;
    pub const FOREACHSTR: u8 = 49;
    pub const IN: u8 = 50;
    pub const ASSERT: u8 = 51;
    pub const SINGLETON: u8 = 52;
    /// `->`
    pub const INTNAME: u8 = 53;
    /// `-->`
    pub const INTNAMER: u8 = 54;
    /// The single-character tokens, in the order of `SINGLE`.
    pub const FIRST_CHAR: u8 = 55;
    pub const SINGLE: &[u8; 25] = b"?[]()+-*/<>|.!:;{},&%^~=@";
    /// `%prec UNARY` (never scanned).
    pub const UNARY: u8 = FIRST_CHAR + 25;
    pub const COUNT: usize = UNARY as usize + 1;

    /// The terminal of a single-character token (`@` included).
    pub const fn char_token(c: u8) -> u8 {
        let mut i = 0;
        while i < SINGLE.len() {
            if SINGLE[i] == c {
                return FIRST_CHAR + i as u8;
            }
            i += 1;
        }
        ILLEGAL
    }
}

/// What a token carries (`CMDlval`).
#[derive(Debug, Clone, PartialEq)]
pub enum TokenValue {
    None,
    /// A keyword: the line it is on (statement nodes made from it take this line).
    Line(u32),
    Int(i32),
    Float(f64),
    /// A string literal, decoded and cut at its first NUL.
    Str(Vec<u8>),
    /// A variable or identifier, spelled as its first occurrence in the file.
    Name(Name),
    /// `@`: the byte the concatenation inserts (0 for none).
    Char(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: u8,
    pub value: TokenValue,
    /// The scanner's line counter when the token was returned (1 + the newlines before it).
    pub line: u32,
}

/// One call of the scanner.
#[derive(Debug, Clone, PartialEq)]
pub enum Lexed {
    Token(Token),
    End,
    /// A string literal with a malformed `\x` or `\c` escape: the scanner returns -1, which the parser
    /// takes as the end of the input.
    BadString,
    /// A `/*` comment without its `*/`.
    UnterminatedComment,
    /// A scanner of another dialect stopped: the message follows the syntax error report.
    Error(String),
}

/// The rules' priorities, in `CMDscan.l`'s order (a lower number wins a tie).
mod rule {
    pub const SPACE: u8 = 0;
    pub const LINE_COMMENT: u8 = 1;
    pub const CR: u8 = 2;
    pub const LF: u8 = 3;
    pub const STRING: u8 = 4;
    pub const TAGGED_STRING: u8 = 5;
    /// The first of the operators (`OPERATORS[i]` is `OPERATOR + i`).
    pub const OPERATOR: u8 = 6;
    /// `NL`, `TAB`, `SPC`, `@`.
    pub const NL: u8 = 31;
    pub const TAB: u8 = 32;
    pub const SPC: u8 = 33;
    pub const CAT: u8 = 34;
    pub const BLOCK_COMMENT: u8 = 35;
    /// Any single-character token.
    pub const SINGLE: u8 = 36;
    /// The first keyword (`KEYWORDS[i]` is `KEYWORD + i`).
    pub const KEYWORD: u8 = 60;
    pub const VAR: u8 = 84;
    pub const IDENT: u8 = 85;
    pub const HEX: u8 = 86;
    pub const INTEGER: u8 = 87;
    pub const FLOAT: u8 = 88;
    pub const ILLEGAL_VAR: u8 = 89;
    pub const ANY: u8 = 90;
}

/// The keywords in `CMDscan.l`'s order, with their terminals (`true` and `false` are integers).
const KEYWORDS: [(&[u8], u8); 24] = [
    (b"in", tok::IN),
    (b"or", tok::CASEOR),
    (b"break", tok::BREAK),
    (b"return", tok::RETURN),
    (b"else", tok::ELSE),
    (b"assert", tok::ASSERT),
    (b"while", tok::WHILE),
    (b"do", tok::DO),
    (b"if", tok::IF),
    (b"foreach$", tok::FOREACHSTR),
    (b"foreach", tok::FOREACH),
    (b"for", tok::FOR),
    (b"continue", tok::CONTINUE),
    (b"function", tok::FUNCTION),
    (b"new", tok::NEW),
    (b"singleton", tok::SINGLETON),
    (b"datablock", tok::DATABLOCK),
    (b"case", tok::CASE),
    (b"switch$", tok::SWITCHSTR),
    (b"switch", tok::SWITCH),
    (b"default", tok::DEFAULT),
    (b"package", tok::PACKAGE),
    (b"true", tok::INTCONST),
    (b"false", tok::INTCONST),
];

/// The rule of a keyword.
fn keyword_rule(word: &[u8]) -> Option<u8> {
    let i = KEYWORDS.iter().position(|(k, _)| *k == word)?;
    Some(rule::KEYWORD + i as u8)
}

/// The two- and three-byte operators and their terminals, in `CMDscan.l`'s order.
const OPERATORS: [(&[u8], u8); 25] = [
    (b"==", tok::EQ),
    (b"!=", tok::NE),
    (b">=", tok::GE),
    (b"<=", tok::LE),
    (b"&&", tok::ANDAND),
    (b"||", tok::OROR),
    (b"::", tok::COLONCOLON),
    (b"--", tok::MINUSMINUS),
    (b"++", tok::PLUSPLUS),
    (b"$=", tok::STREQ),
    (b"!$=", tok::STRNE),
    (b"<<", tok::SHL),
    (b">>", tok::SHR),
    (b"+=", tok::PLASN),
    (b"-=", tok::MIASN),
    (b"*=", tok::MLASN),
    (b"/=", tok::DVASN),
    (b"%=", tok::MODASN),
    (b"&=", tok::ANDASN),
    (b"^=", tok::XORASN),
    (b"|=", tok::ORASN),
    (b"<<=", tok::SLASN),
    (b">>=", tok::SRASN),
    (b"->", tok::INTNAME),
    (b"-->", tok::INTNAMER),
];

/// The rule of `OPERATORS[i]`.
fn operator_rule(i: usize) -> u8 {
    rule::OPERATOR + i as u8
}

/// The entry of `OPERATORS` an operator rule matched.
fn operator_of_rule(r: u8) -> Option<&'static (&'static [u8], u8)> {
    OPERATORS.get(usize::from(r.checked_sub(rule::OPERATOR)?))
}

fn is_letter(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_tail(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The result of one rule family on the text at the scan position: its longest match (rule, length), if any.
#[derive(Clone, Copy)]
struct Family {
    accept: Option<(u8, usize)>,
}

pub struct Lexer<'a> {
    /// The input, up to its first NUL.
    src: &'a [u8],
    /// The first byte of the next scan.
    pos: usize,
    /// `CMDgetc`'s position: the end of the lines read so far.
    scan_index: usize,
    line: u32,
    names: HashMap<Vec<u8>, Name>,
    /// Calls of [`Lexer::next_token`] so far.
    calls: u32,
    language: Language,
}

impl<'a> Lexer<'a> {
    /// A scanner of Torque3D's tokens.
    pub fn new(src: &'a [u8]) -> Lexer<'a> {
        Lexer::with_language(src, Language::Torque3d)
    }

    /// A scanner of the given language's tokens.
    pub fn with_language(src: &'a [u8], language: Language) -> Lexer<'a> {
        let end = src.iter().position(|&b| b == 0).unwrap_or(src.len());
        Lexer {
            src: &src[..end],
            pos: 0,
            scan_index: 0,
            line: 1,
            names: HashMap::new(),
            calls: 0,
            language,
        }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// Whether `OPERATORS[i]` is one of this language's.
    fn has_operator(&self, i: usize) -> bool {
        self.language == Language::Torque3d
            || OPERATORS
                .get(i)
                .is_some_and(|(_, t)| *t != tok::INTNAME && *t != tok::INTNAMER)
    }

    /// The rule of a keyword of this language.
    fn keyword(&self, word: &[u8]) -> Option<u8> {
        if self.language == Language::Tge14 && TORQUE3D_KEYWORDS.contains(&word) {
            return None;
        }
        keyword_rule(word)
    }

    pub fn source(&self) -> &'a [u8] {
        self.src
    }

    pub fn scan_index(&self) -> usize {
        self.scan_index
    }

    /// How many tokens the parser has asked for (the end included).
    pub fn tokens(&self) -> u32 {
        self.calls
    }

    /// `lineIndex`: 1 + the newlines scanned so far.
    pub fn line(&self) -> u32 {
        self.line
    }

    /// The token the parser sees at the end of the input.
    pub fn end_token(&self) -> Token {
        Token {
            kind: tok::END,
            value: TokenValue::None,
            line: self.line,
        }
    }

    /// The `k`-th byte of the current scan, None at the end of the input. Looking at a byte past the lines read so
    /// far reads up to the end of its line (`YY_INPUT`).
    fn at(&mut self, k: usize) -> Option<u8> {
        let p = self.pos + k;
        let b = self.src.get(p).copied()?;
        if p >= self.scan_index {
            self.scan_index = self
                .src
                .get(p..)
                .and_then(|rest| rest.iter().position(|&c| c == b'\n'))
                .map_or(self.src.len(), |nl| p + nl + 1);
        }
        Some(b)
    }

    /// Moves the scan position past `n` bytes of text, returning the text.
    fn consume(&mut self, n: usize) -> Vec<u8> {
        let end = (self.pos + n).min(self.src.len());
        let text = self.src.get(self.pos..end).unwrap_or_default().to_vec();
        self.pos = end;
        text
    }

    /// Runs the rule families, each looking at bytes only while it can go on, and takes the longest match (the
    /// earlier rule on a tie). Returns the matched rule and length, or None at the end of the input.
    fn scan(&mut self) -> Option<(u8, usize)> {
        let first = self.at(0)?;
        let families = [
            self.whitespace(first),
            self.comment(first),
            self.string(first),
            self.operator(first),
            self.word(first),
            self.variable(first),
            self.number(first),
        ];
        // The CR and LF rules, or the one for any byte but a newline, match the first byte.
        let mut best: (u8, usize) = match first {
            b'\r' => (rule::CR, 1),
            b'\n' => (rule::LF, 1),
            _ => (rule::ANY, 1),
        };
        for f in &families {
            if let Some((r, len)) = f.accept
                && (len > best.1 || (len == best.1 && r < best.0))
            {
                best = (r, len);
            }
        }
        Some(best)
    }

    fn whitespace(&mut self, first: u8) -> Family {
        let space = |b: u8| matches!(b, b' ' | b'\t' | 0x0b | 0x0c);
        if !space(first) {
            return Family { accept: None };
        }
        let mut n = 1;
        while self.at(n).is_some_and(space) {
            n += 1;
        }
        Family {
            accept: Some((rule::SPACE, n)),
        }
    }

    fn comment(&mut self, first: u8) -> Family {
        if first != b'/' {
            return Family { accept: None };
        }
        if self.at(1) == Some(b'*') {
            // `/*` can go no further: the rest of the comment is read by the rule's action.
            return Family {
                accept: Some((rule::BLOCK_COMMENT, 2)),
            };
        }
        if self.at(1) != Some(b'/') {
            return Family { accept: None };
        }
        let mut n = 2;
        while self.at(n).is_some_and(|b| b != b'\n' && b != b'\r') {
            n += 1;
        }
        Family {
            accept: Some((rule::LINE_COMMENT, n)),
        }
    }

    fn string(&mut self, first: u8) -> Family {
        if first != b'"' && first != b'\'' {
            return Family { accept: None };
        }
        let r = if first == b'"' {
            rule::STRING
        } else {
            rule::TAGGED_STRING
        };
        let mut n = 1;
        loop {
            match self.at(n) {
                Some(b) if b == first => {
                    return Family {
                        accept: Some((r, n + 1)),
                    };
                }
                Some(b'\\') => match self.at(n + 1) {
                    Some(b) if b != b'\n' => n += 2,
                    _ => {
                        return Family { accept: None };
                    }
                },
                Some(b) if b != b'\n' && b != b'\r' => n += 1,
                _ => {
                    return Family { accept: None };
                }
            }
        }
    }

    fn operator(&mut self, first: u8) -> Family {
        // Single characters (and `@`).
        let single = tok::SINGLE.contains(&first);
        let single_rule = if first == b'@' {
            rule::CAT
        } else {
            rule::SINGLE
        };
        let mut accept = single.then_some((single_rule, 1));
        let starts = OPERATORS
            .iter()
            .enumerate()
            .any(|(i, (text, _))| self.has_operator(i) && text.first() == Some(&first));
        // `$` alone matches nothing here but starts `$=`.
        if !starts {
            return Family { accept };
        }
        let second = self.at(1);
        let mut two = false;
        for (i, (text, _)) in OPERATORS.iter().enumerate() {
            if !self.has_operator(i)
                || text.first() != Some(&first)
                || second != text.get(1).copied()
            {
                continue;
            }
            two = true;
            if text.len() == 2 {
                accept = Some((operator_rule(i), 2));
            }
        }
        // Does a three-byte operator go on from these two?
        let longer = OPERATORS.iter().enumerate().any(|(i, (t, _))| {
            self.has_operator(i)
                && t.len() == 3
                && t.first() == Some(&first)
                && t.get(1).copied() == second
        });
        if two && longer {
            let third = self.at(2);
            for (i, (text, _)) in OPERATORS.iter().enumerate() {
                if self.has_operator(i)
                    && text.len() == 3
                    && text.first() == Some(&first)
                    && text.get(1).copied() == second
                    && text.get(2).copied() == third
                {
                    accept = Some((operator_rule(i), 3));
                }
            }
        }
        Family { accept }
    }

    fn word(&mut self, first: u8) -> Family {
        if !is_letter(first) {
            return Family { accept: None };
        }
        let mut n = 1;
        let mut text = vec![first];
        while let Some(b) = self.at(n).filter(|&b| is_tail(b)) {
            text.push(b);
            n += 1;
        }
        let dollar = match self.language {
            Language::Torque3d => matches!(text.as_slice(), b"switch" | b"foreach"),
            Language::Tge14 => text == b"switch",
        };
        if dollar && self.at(n) == Some(b'$') {
            // `switch$` (and Torque3D's `foreach$`) can go no further.
            text.push(b'$');
            return Family {
                accept: self.keyword(&text).map(|r| (r, n + 1)),
            };
        }
        let r = match text.as_slice() {
            b"NL" => rule::NL,
            b"TAB" => rule::TAB,
            b"SPC" => rule::SPC,
            other => self.keyword(other).unwrap_or(rule::IDENT),
        };
        Family {
            accept: Some((r, n)),
        }
    }

    fn variable(&mut self, first: u8) -> Family {
        if first != b'$' && first != b'%' {
            return Family { accept: None };
        }
        let second = self.at(1);
        if second.is_some_and(is_letter) {
            // [$%][A-Za-z_] then [A-Za-z0-9_:]*, accepted where the last byte is not ':'.
            let mut n = 2;
            let mut last_good = 2;
            while let Some(b) = self.at(n).filter(|&b| is_tail(b) || b == b':') {
                n += 1;
                if b != b':' {
                    last_good = n;
                }
            }
            return Family {
                accept: Some((rule::VAR, last_good)),
            };
        }
        if second.is_some_and(|b| b.is_ascii_digit()) {
            // The illegal form: [$%][0-9]+[A-Za-z_]([A-Za-z0-9_:]*[A-Za-z0-9_])?
            let mut n = 2;
            while self.at(n).is_some_and(|b| b.is_ascii_digit()) {
                n += 1;
            }
            if !self.at(n).is_some_and(is_letter) {
                return Family { accept: None };
            }
            n += 1;
            let mut last_good = n;
            while let Some(b) = self.at(n).filter(|&b| is_tail(b) || b == b':') {
                n += 1;
                if b != b':' {
                    last_good = n;
                }
            }
            return Family {
                accept: Some((rule::ILLEGAL_VAR, last_good)),
            };
        }
        Family { accept: None }
    }

    fn number(&mut self, first: u8) -> Family {
        if first == b'.' && self.language == Language::Torque3d {
            return self.fraction();
        }
        if !first.is_ascii_digit() {
            return Family { accept: None };
        }
        let digits_from = |lexer: &mut Lexer<'_>, mut n: usize| {
            while lexer.at(n).is_some_and(|b| b.is_ascii_digit()) {
                n += 1;
            }
            n
        };
        // Hex: 0[xX][0-9a-fA-F]+
        if first == b'0' && matches!(self.at(1), Some(b'x' | b'X')) {
            let mut n = 2;
            while self.at(n).is_some_and(|b| b.is_ascii_hexdigit()) {
                n += 1;
            }
            // A decimal reading of "0" is shorter; the hex rule wins only when it matched.
            return Family {
                accept: Some(if n > 2 {
                    (rule::HEX, n)
                } else {
                    (rule::INTEGER, 1)
                }),
            };
        }
        let int_end = digits_from(self, 1);
        let mut accept = Some((rule::INTEGER, int_end));
        // Float: digits '.' digits [exponent], or digits exponent.
        let mut mantissa_end = int_end;
        if self.at(int_end) == Some(b'.') {
            let frac_end = digits_from(self, int_end + 1);
            if frac_end > int_end + 1 {
                accept = Some((rule::FLOAT, frac_end));
                mantissa_end = frac_end;
            } else {
                return Family { accept };
            }
        }
        if matches!(self.at(mantissa_end), Some(b'e' | b'E')) {
            let mut n = mantissa_end + 1;
            if matches!(self.at(n), Some(b'+' | b'-')) {
                n += 1;
            }
            let exp_end = digits_from(self, n);
            if exp_end > n {
                accept = Some((rule::FLOAT, exp_end));
            }
        }
        Family { accept }
    }

    /// A float without an integer part: `.` digits, then an optional exponent.
    fn fraction(&mut self) -> Family {
        let mut n = 1;
        while self.at(n).is_some_and(|b| b.is_ascii_digit()) {
            n += 1;
        }
        if n == 1 {
            // `.` alone is the operator's.
            return Family { accept: None };
        }
        let mut accept = Some((rule::FLOAT, n));
        if matches!(self.at(n), Some(b'e' | b'E')) {
            let mut e = n + 1;
            if matches!(self.at(e), Some(b'+' | b'-')) {
                e += 1;
            }
            let mut end = e;
            while self.at(end).is_some_and(|b| b.is_ascii_digit()) {
                end += 1;
            }
            if end > e {
                accept = Some((rule::FLOAT, end));
            }
        }
        Family { accept }
    }

    fn intern(&mut self, text: Vec<u8>) -> Name {
        let key = text.to_ascii_lowercase();
        self.names
            .entry(key)
            .or_insert_with(|| Name::from(text))
            .clone()
    }

    /// The next token (`yylex`).
    pub fn next_token(&mut self) -> Lexed {
        self.calls = self.calls.saturating_add(1);
        loop {
            let Some((r, len)) = self.scan() else {
                return Lexed::End;
            };
            let text = self.consume(len);
            let token =
                |kind: u8, value: TokenValue, line: u32| Lexed::Token(Token { kind, value, line });
            let line = self.line;
            return match r {
                rule::SPACE | rule::LINE_COMMENT | rule::CR => continue,
                rule::LF => {
                    self.line = self.line.saturating_add(1);
                    continue;
                }
                rule::STRING | rule::TAGGED_STRING => {
                    let body = text
                        .get(1..text.len().saturating_sub(1))
                        .unwrap_or_default();
                    let (decoded, ok) = collapse_escape_checked(body);
                    if !ok {
                        return Lexed::BadString;
                    }
                    let kind = if r == rule::STRING {
                        tok::STRATOM
                    } else {
                        tok::TAGATOM
                    };
                    token(kind, TokenValue::Str(decoded), line)
                }
                rule::NL..=rule::CAT => {
                    let byte = match r {
                        rule::NL => b'\n',
                        rule::TAB => b'\t',
                        rule::SPC => b' ',
                        _ => 0,
                    };
                    token(tok::char_token(b'@'), TokenValue::Char(byte), line)
                }
                rule::BLOCK_COMMENT => {
                    if !self.skip_block_comment() {
                        return Lexed::UnterminatedComment;
                    }
                    continue;
                }
                rule::SINGLE => token(
                    tok::char_token(text.first().copied().unwrap_or(0)),
                    TokenValue::None,
                    line,
                ),
                rule::VAR => {
                    let name = self.intern(text);
                    token(tok::VAR, TokenValue::Name(name), line)
                }
                rule::IDENT if self.language == Language::Tge14 && text == b"namespace" => {
                    token(tok::ILLEGAL, TokenValue::Line(line), line)
                }
                rule::IDENT => {
                    let name = self.intern(text);
                    token(tok::IDENT, TokenValue::Name(name), line)
                }
                rule::HEX => token(
                    tok::INTCONST,
                    TokenValue::Int(hex_value(text.get(2..).unwrap_or_default())),
                    line,
                ),
                rule::INTEGER => token(tok::INTCONST, TokenValue::Int(atoi(&text)), line),
                rule::FLOAT => {
                    let value = to_f64(to_f32(atof(&text)));
                    token(tok::FLTCONST, TokenValue::Float(value), line)
                }
                _ if text == b"true" || text == b"false" => token(
                    tok::INTCONST,
                    TokenValue::Int(i32::from(text == b"true")),
                    line,
                ),
                _ => {
                    if let Some(op) = operator_of_rule(r).filter(|_| r < rule::NL) {
                        token(op.1, TokenValue::None, line)
                    } else if let Some(k) = r
                        .checked_sub(rule::KEYWORD)
                        .and_then(|i| KEYWORDS.get(usize::from(i)))
                        .filter(|_| r < rule::VAR)
                    {
                        token(k.1, TokenValue::Line(line), line)
                    } else {
                        token(tok::ILLEGAL, TokenValue::None, line)
                    }
                }
            };
        }
    }
}

impl Lexer<'_> {
    /// Reads the rest of a `/*` comment up to and including its `*/`, a byte at a time, counting its line feeds.
    /// False at the end of the input.
    fn skip_block_comment(&mut self) -> bool {
        let mut star = false;
        loop {
            let Some(b) = self.at(0) else {
                return false;
            };
            self.consume(1);
            if b == b'\n' {
                self.line = self.line.saturating_add(1);
            }
            if star && b == b'/' {
                return true;
            }
            star = b == b'*';
        }
    }
}

/// The value of a hex literal as `CMDscan.l`'s `Sc_ScanHex` takes it, `sscanf("%x")` into an `S32`: the digits are read as an
/// unsigned 64-bit number (`strtoull`'s rule: saturating at its largest) and the low 32 bits are stored, as common C libraries
/// (glibc among them) do; C itself leaves an overflowing `%x` undefined.
fn hex_value(digits: &[u8]) -> i32 {
    let mut v: u64 = 0;
    for &d in digits {
        let Some(x) = char::from(d).to_digit(16) else {
            break;
        };
        v = v
            .checked_mul(16)
            .and_then(|v| v.checked_add(u64::from(x)))
            .unwrap_or(u64::MAX);
    }
    v as u32 as i32
}
