//! FGD files (a map editor's entity definitions), read at run time from a user's own install when present: each class's
//! keyvalues with their types and defaults, its spawnflags with their default bits, and its input and output names. Nothing
//! from an FGD is stored, shipped or written out; every handler has its own coded defaults ([`crate::defaults`]) for when no
//! FGD is read, and a map runs the same without one.
//!
//! **Grammar** (a hand-written lexer and recursive-descent parser): `//` comments to the end of a line; quoted strings, joined
//! by `+` across lines; `@include "file"`; `@<Kind>Class` headers (any kind, any case: `BaseClass`, `PointClass`, `SolidClass`,
//! `NPCClass`, `FilterClass`, `KeyFrameClass`, `MoveClass`, and `ExtendClass`, which adds to a class defined elsewhere) with
//! properties `name(args)` of which only `base(A, B)` is kept, then `= classname`, an optional `: "description"` and a
//! bracketed body. A body holds keys `name(type) [readonly] [report] : "label" : default : "help"` (any field may be left
//! out, `::` leaves one empty), an optional `= [ value : "label" [: default] ... ]` list for `choices` and `flags` keys,
//! and `input name(type)` and `output name(type)` lines. Types are kept as text (case-insensitive; an unknown type is never
//! refused). Any other `@` section (`@mapsize`, `@MaterialExclusion`, `@AutoVisGroup`, ...) is skipped with its bracketed body.
//! Labels, descriptions and help texts are read past and not kept.
//!
//! **Includes** resolve beside the including file, through [`FgdSource`]: a plain file name only (no separator, no `..`, no
//! drive), each file once, [`MAX_INCLUDE_DEPTH`] deep at most. A missing include is a warning.
//!
//! **Classes.** A second class of a name already read replaces the first (the last one defined is used); `@ExtendClass`
//! entries are applied after every file is read, in order, and are ignored for a class that does not exist. A class's
//! effective definition ([`EntityDefs`], flattened when first asked for, within caps on entries and work) is its bases' (depth first, in the listed order, each base once, so a diamond adds
//! nothing twice) overlaid by its own: a later key, input or output of the same name replaces the earlier in its place; flags
//! keys (spawnflags) merge bit by bit. A base cycle is cut where it closes, and bases nest at most [`MAX_BASE_DEPTH`] deep.
//!
//! **Untrusted input.** The bytes of each file, of all files together, the tokens, classes, entries of a class, choices of a
//! key, bases of a class, string lengths and the parser's steps are capped (the `MAX_*` constants). At a cap the reader stops
//! reading that file, keeps what it has and records one warning. Bytes are decoded as UTF-8 where they are, else byte by byte
//! as Latin-1. Nothing here panics on any input (the crate's fuzz test).
//!
//! # Maintaining
//!
//! **Files on disk.** `DirSource` and `read_plain` are `torque_resource::plain`: a file is opened once without following a link, the
//! length comes from the open file, and the read stops one byte past `MAX_FILE_BYTES` (`tests/fgd_files.rs::a_file_over_the_cap_at_read_time_is_refused_and_one_at_the_cap_is_read`,
//! `folders_and_missing_names_are_not_files`, and `a_link_is_refused` on Unix only).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The largest FGD file read.
pub const MAX_FILE_BYTES: usize = 4 << 20;
/// The most bytes read over a file and all its includes.
pub const MAX_TOTAL_BYTES: usize = 16 << 20;
/// The most files read (the first and its includes).
pub const MAX_FILES: usize = 64;
/// The deepest include chain.
pub const MAX_INCLUDE_DEPTH: usize = 8;
/// The most tokens of all files together.
pub const MAX_TOKENS: usize = 4 << 20;
/// The most classes kept.
pub const MAX_CLASSES: usize = 8192;
/// The most keys, inputs and outputs of one class (each).
pub const MAX_ENTRIES: usize = 1024;
/// The most choices of one key.
pub const MAX_CHOICES: usize = 1024;
/// The most bases of one class.
pub const MAX_BASES: usize = 64;
/// The longest string or word kept, in bytes (a longer one is cut).
pub const MAX_STRING: usize = 4096;
/// The parser's steps over all files.
pub const MAX_STEPS: usize = 16 << 20;
/// The deepest base chain flattened.
pub const MAX_BASE_DEPTH: usize = 32;
/// The deepest bracket nesting a skipped section may have.
pub const MAX_NESTING: usize = 64;

/// Where an FGD and its includes come from: a file by its plain name.
pub trait FgdSource {
    /// The bytes of `name` (already checked to be a plain file name), None when it is missing or refused.
    fn read(&self, name: &str) -> Option<Vec<u8>>;
}

/// The files of one folder (a game's `bin` folder): a plain regular file only (a link is refused), at most
/// [`MAX_FILE_BYTES`].
#[derive(Debug, Clone)]
pub struct DirSource {
    pub dir: PathBuf,
}

impl FgdSource for DirSource {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        if !plain_name(name) {
            return None;
        }
        read_plain(&self.dir.join(name), MAX_FILE_BYTES as u64)
    }
}

/// The plain file at `path`, at most `max` bytes ([`torque_resource::plain::read_plain`]): no link, no folder, the length from
/// the open file, so a file swapped or grown after a look is refused rather than read whole.
pub fn read_plain(path: &Path, max: u64) -> Option<Vec<u8>> {
    torque_resource::plain::read_plain(path, max, None).ok()
}

/// Files held in memory by name (tests, and a host that read them itself).
#[derive(Debug, Clone, Default)]
pub struct MemorySource(pub Vec<(String, Vec<u8>)>);

impl FgdSource for MemorySource {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, b)| b.clone())
    }
}

/// Whether `name` is a plain file name: not empty, no separator, no `..`, no drive or stream colon, no control byte.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && !name.contains("..")
        && !name
            .bytes()
            .any(|c| matches!(c, b'/' | b'\\' | b':') || c < 0x20)
}

/// A key's type as the file names it, matched without case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyType {
    Integer,
    Float,
    String,
    Boolean,
    Choices,
    Flags,
    Color255,
    Vector,
    /// `target_source`, `target_destination`, `target_name_or_class` and the other entity-naming types.
    Target,
    /// Any other type, as written (lowercase).
    Other(String),
}

impl KeyType {
    pub fn parse(name: &str) -> KeyType {
        let n = name.trim().to_ascii_lowercase();
        match n.as_str() {
            "integer" | "int" => KeyType::Integer,
            "float" => KeyType::Float,
            "string" => KeyType::String,
            "boolean" | "bool" => KeyType::Boolean,
            "choices" => KeyType::Choices,
            "flags" => KeyType::Flags,
            "color255" => KeyType::Color255,
            "vector" | "origin" | "vecline" => KeyType::Vector,
            _ if n.starts_with("target_") => KeyType::Target,
            _ => KeyType::Other(n),
        }
    }
}

/// A keyvalue read by its declared type ([`EntityDefs::typed`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Typed {
    Int(i64),
    Float(f64),
    Bool(bool),
    Color([u8; 3]),
    Vector([f32; 3]),
    Str(String),
}

impl KeyType {
    /// `text` read as this type, leniently (a number from its leading part, a color's missing channels 255).
    pub fn read(&self, text: &str) -> Typed {
        use crate::value::{atof, atoi, boolean, color255, vec3};
        match self {
            KeyType::Integer | KeyType::Flags => Typed::Int(atoi(text)),
            KeyType::Float => Typed::Float(atof(text)),
            KeyType::Boolean => Typed::Bool(boolean(text)),
            KeyType::Color255 => Typed::Color(color255(text)),
            KeyType::Vector => Typed::Vector(vec3(text).unwrap_or([0.0; 3])),
            _ => Typed::Str(text.to_string()),
        }
    }
}

/// One choice of a `choices` key, or one bit of a `flags` key.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub value: String,
    /// A flag's default (`1` set); always false for a plain choice.
    pub default: bool,
}

/// A keyvalue's definition.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyDef {
    pub name: String,
    /// `name` in lowercase (what merges match on).
    pub lower: String,
    pub ty: KeyType,
    pub default: Option<String>,
    pub choices: Vec<Choice>,
}

/// An input's or output's definition: its name and its value type (as written, lowercase).
#[derive(Debug, Clone, PartialEq)]
pub struct IoDef {
    pub name: String,
    /// `name` in lowercase.
    pub lower: String,
    pub ty: String,
}

/// A class as one file defines it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClassDef {
    pub name: String,
    /// The section's kind, lowercase without the `@` (`pointclass`, `baseclass`, ...).
    pub kind: String,
    pub bases: Vec<String>,
    pub keys: Vec<Rc<KeyDef>>,
    pub inputs: Vec<Rc<IoDef>>,
    pub outputs: Vec<Rc<IoDef>>,
}

/// What the files held: the classes by lowercase name, and what went wrong.
#[derive(Debug, Clone, Default)]
pub struct Fgd {
    pub classes: HashMap<String, ClassDef>,
    /// The files read, in order.
    pub files: Vec<String>,
    /// One line for each cap reached, missing include, cycle or refusal.
    pub warnings: Vec<String>,
    extends: Vec<ClassDef>,
}

/// The shared budgets of one read over all files.
struct Budget {
    bytes: usize,
    tokens: usize,
    steps: usize,
    seen: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Str(String),
    Word(String),
    Punct(u8),
}

/// Appends `t` to `s`, keeping `s` within [`MAX_STRING`] bytes (cut at a character boundary).
fn push_capped(s: &mut String, t: &str) {
    for c in t.chars() {
        if s.len() + c.len_utf8() > MAX_STRING {
            return;
        }
        s.push(c);
    }
}

fn decode(b: &[u8]) -> String {
    let b = &b[..b.len().min(MAX_STRING)];
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    }
}

/// The tokens of one file, or as many as the budget allows (and whether it ran out).
fn lex(b: &[u8], budget: &mut Budget) -> (Vec<Tok>, bool) {
    let mut out = Vec::new();
    let mut i = 0;
    let n = b.len();
    while i < n {
        if budget.tokens >= MAX_TOKENS {
            return (out, true);
        }
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < n && b[j] != b'"' {
                j += 1;
            }
            out.push(Tok::Str(decode(&b[start..j])));
            budget.tokens += 1;
            i = (j + 1).min(n);
            continue;
        }
        if matches!(
            c,
            b'@' | b'(' | b')' | b'[' | b']' | b'=' | b':' | b',' | b'+'
        ) {
            out.push(Tok::Punct(c));
            budget.tokens += 1;
            i += 1;
            continue;
        }
        let start = i;
        while i < n {
            let d = b[i];
            if d.is_ascii_whitespace()
                || matches!(
                    d,
                    b'"' | b'@' | b'(' | b')' | b'[' | b']' | b'=' | b':' | b',' | b'+'
                )
                || (d == b'/' && b.get(i + 1) == Some(&b'/'))
            {
                break;
            }
            i += 1;
        }
        out.push(Tok::Word(decode(&b[start..i])));
        budget.tokens += 1;
    }
    (out, false)
}

/// The parser over one file's tokens.
struct Parser<'a> {
    toks: &'a [Tok],
    at: usize,
    out_of_steps: bool,
    steps: &'a mut usize,
}

impl Parser<'_> {
    fn step(&mut self) -> bool {
        *self.steps += 1;
        if *self.steps > MAX_STEPS {
            self.out_of_steps = true;
        }
        !self.out_of_steps
    }
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        if t.is_some() {
            self.at += 1;
        }
        t
    }
    fn is(&self, p: u8) -> bool {
        self.peek() == Some(&Tok::Punct(p))
    }
    fn eat(&mut self, p: u8) -> bool {
        if self.is(p) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self) -> Option<String> {
        match self.peek() {
            Some(Tok::Word(w)) => {
                let w = w.clone();
                self.at += 1;
                Some(w)
            }
            _ => None,
        }
    }
    /// A string, `+` joining the next ones.
    fn string(&mut self) -> Option<String> {
        let Some(Tok::Str(s)) = self.peek() else {
            return None;
        };
        let mut s = s.clone();
        self.at += 1;
        while self.is(b'+') && matches!(self.toks.get(self.at + 1), Some(Tok::Str(_))) {
            if !self.step() {
                break;
            }
            self.at += 1;
            if let Some(Tok::Str(t)) = self.next() {
                push_capped(&mut s, &t);
            }
        }
        Some(s)
    }
    /// A string or a word (a default value).
    fn value(&mut self) -> Option<String> {
        if let Some(s) = self.string() {
            return Some(s);
        }
        self.word()
    }
    /// Skips a balanced group that opens at the current token (`(` or `[`); true when one was skipped.
    fn skip_group(&mut self) -> bool {
        let (open, close) = match self.peek() {
            Some(Tok::Punct(b'(')) => (b'(', b')'),
            Some(Tok::Punct(b'[')) => (b'[', b']'),
            _ => return false,
        };
        let mut depth = 0usize;
        while let Some(t) = self.next() {
            if !self.step() {
                return true;
            }
            match t {
                Tok::Punct(p) if p == open => {
                    depth += 1;
                    if depth > MAX_NESTING {
                        self.out_of_steps = true;
                        return true;
                    }
                }
                Tok::Punct(p) if p == close => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return true;
                    }
                }
                _ => {}
            }
        }
        true
    }
    /// Words inside parentheses, split at commas (`base(A, B)`); the parentheses are consumed.
    fn paren_list(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.eat(b'(') {
            return out;
        }
        let mut cur = String::new();
        while let Some(t) = self.next() {
            if !self.step() {
                break;
            }
            match t {
                Tok::Punct(b')') => break,
                Tok::Punct(b',') => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                Tok::Word(w) | Tok::Str(w) => {
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    push_capped(&mut cur, &w);
                }
                Tok::Punct(_) => {}
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
}

/// What one top-level pass found: classes, extensions and includes, in order.
enum Item {
    Class(ClassDef),
    Extend(ClassDef),
    Include(String),
}

fn parse_body(p: &mut Parser<'_>, class: &mut ClassDef) {
    // Within `[ ... ]`; the opening bracket is consumed.
    while let Some(t) = p.peek().cloned() {
        if !p.step() {
            return;
        }
        match t {
            Tok::Punct(b']') => {
                p.at += 1;
                return;
            }
            Tok::Punct(b'@') => return,
            Tok::Word(w) if w.eq_ignore_ascii_case("input") || w.eq_ignore_ascii_case("output") => {
                p.at += 1;
                let Some(name) = p.word() else { continue };
                let ty = p.paren_list().join(" ").to_ascii_lowercase();
                if p.eat(b':') {
                    p.string();
                }
                let list = if w.eq_ignore_ascii_case("input") {
                    &mut class.inputs
                } else {
                    &mut class.outputs
                };
                if list.len() < MAX_ENTRIES {
                    list.push(Rc::new(IoDef {
                        lower: name.to_ascii_lowercase(),
                        name,
                        ty,
                    }));
                }
            }
            Tok::Word(name) => {
                p.at += 1;
                if !p.is(b'(') {
                    continue;
                }
                let ty = p.paren_list().join(" ");
                // Modifiers after the type.
                while let Some(Tok::Word(m)) = p.peek() {
                    if m.eq_ignore_ascii_case("readonly") || m.eq_ignore_ascii_case("report") {
                        p.at += 1;
                    } else {
                        break;
                    }
                }
                let mut default = None;
                if p.eat(b':') {
                    p.string(); // the label
                    if p.eat(b':') {
                        if !p.is(b':') {
                            default = p.value();
                        }
                        if p.eat(b':') {
                            p.string(); // the help
                        }
                    }
                }
                let mut choices = Vec::new();
                if p.eat(b'=') {
                    if p.eat(b'[') {
                        parse_choices(p, &mut choices);
                    } else {
                        p.skip_group();
                    }
                }
                let ty = KeyType::parse(&ty);
                if class.keys.len() < MAX_ENTRIES {
                    class.keys.push(Rc::new(KeyDef {
                        lower: name.to_ascii_lowercase(),
                        name,
                        ty,
                        default,
                        choices,
                    }));
                }
            }
            Tok::Punct(b'[' | b'(') => {
                p.skip_group();
            }
            _ => {
                p.at += 1;
            }
        }
    }
}

fn parse_choices(p: &mut Parser<'_>, out: &mut Vec<Choice>) {
    while let Some(t) = p.peek().cloned() {
        if !p.step() {
            return;
        }
        match t {
            Tok::Punct(b']') => {
                p.at += 1;
                return;
            }
            Tok::Punct(b'@') => return,
            Tok::Word(_) | Tok::Str(_) => {
                let value = p.value().unwrap_or_default();
                let mut default = false;
                if p.eat(b':') {
                    p.string();
                    if p.eat(b':')
                        && let Some(d) = p.value()
                    {
                        default = crate::value::atoi(&d) != 0;
                    }
                }
                if out.len() < MAX_CHOICES {
                    out.push(Choice { value, default });
                }
            }
            Tok::Punct(b'[' | b'(') => {
                p.skip_group();
            }
            _ => p.at += 1,
        }
    }
}

/// One file's items (and whether a cap stopped it).
fn parse_file(toks: &[Tok], steps: &mut usize) -> (Vec<Item>, bool) {
    let mut p = Parser {
        toks,
        at: 0,
        out_of_steps: false,
        steps,
    };
    let mut items = Vec::new();
    while p.peek().is_some() {
        if !p.step() {
            break;
        }
        if !p.eat(b'@') {
            p.at += 1;
            continue;
        }
        let Some(kind) = p.word() else { continue };
        let lower = kind.to_ascii_lowercase();
        if lower == "include" {
            if let Some(f) = p.string() {
                items.push(Item::Include(f));
            }
            continue;
        }
        if !lower.ends_with("class") {
            // An unknown section: skip to its bracketed body (or the next section).
            while let Some(t) = p.peek().cloned() {
                if !p.step() {
                    break;
                }
                match t {
                    Tok::Punct(b'@') => break,
                    Tok::Punct(b'[') => {
                        p.skip_group();
                        break;
                    }
                    Tok::Punct(b'(') => {
                        p.skip_group();
                    }
                    _ => p.at += 1,
                }
            }
            continue;
        }
        let mut class = ClassDef {
            kind: lower.clone(),
            ..ClassDef::default()
        };
        // Properties up to `=`.
        while let Some(t) = p.peek().cloned() {
            if !p.step() {
                break;
            }
            match t {
                Tok::Punct(b'=') => break,
                Tok::Punct(b'@' | b'[') => break,
                Tok::Word(w) => {
                    p.at += 1;
                    if p.is(b'(') {
                        if w.eq_ignore_ascii_case("base") {
                            for b in p.paren_list() {
                                if class.bases.len() < MAX_BASES {
                                    class.bases.push(b);
                                }
                            }
                        } else {
                            p.skip_group();
                        }
                    }
                }
                Tok::Punct(b'(') => {
                    p.skip_group();
                }
                _ => p.at += 1,
            }
        }
        if !p.eat(b'=') {
            continue;
        }
        let Some(name) = p.word().or_else(|| p.string()) else {
            continue;
        };
        class.name = name;
        if p.eat(b':') {
            p.string();
        }
        if !p.eat(b'[') {
            continue;
        }
        parse_body(&mut p, &mut class);
        if lower == "extendclass" {
            items.push(Item::Extend(class));
        } else {
            items.push(Item::Class(class));
        }
    }
    let stopped = p.out_of_steps;
    (items, stopped)
}

impl Fgd {
    /// Reads `main` and its includes from `source` (module docs).
    pub fn load(source: &dyn FgdSource, main: &str) -> Fgd {
        let mut fgd = Fgd::default();
        let mut budget = Budget {
            bytes: 0,
            tokens: 0,
            steps: 0,
            seen: HashSet::new(),
        };
        fgd.read_file(source, main, 0, &mut budget);
        fgd.apply_extends();
        fgd
    }

    /// One file's text alone (includes are warnings: there is nowhere to read them from).
    pub fn parse(bytes: &[u8]) -> Fgd {
        let src = MemorySource(vec![("main.fgd".into(), bytes.to_vec())]);
        Fgd::load(&src, "main.fgd")
    }

    fn warn(&mut self, w: String) {
        if self.warnings.len() < 256 && !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    fn read_file(&mut self, source: &dyn FgdSource, name: &str, depth: usize, b: &mut Budget) {
        if depth > MAX_INCLUDE_DEPTH {
            self.warn(format!(
                "{name}: includes nest deeper than {MAX_INCLUDE_DEPTH}"
            ));
            return;
        }
        if !plain_name(name) {
            self.warn(format!("refused include {name:?}: not a plain file name"));
            return;
        }
        let key = name.to_ascii_lowercase();
        if !b.seen.insert(key) {
            return;
        }
        if self.files.len() >= MAX_FILES {
            self.warn(format!("{name}: more than {MAX_FILES} files"));
            return;
        }
        let Some(bytes) = source.read(name) else {
            self.warn(format!("missing include {name}"));
            return;
        };
        if bytes.len() > MAX_FILE_BYTES {
            self.warn(format!("{name}: larger than {MAX_FILE_BYTES} bytes"));
            return;
        }
        if b.bytes + bytes.len() > MAX_TOTAL_BYTES {
            self.warn(format!(
                "{name}: past {MAX_TOTAL_BYTES} bytes over all files"
            ));
            return;
        }
        b.bytes += bytes.len();
        self.files.push(name.to_string());
        let (toks, cut) = lex(&bytes, b);
        if cut {
            self.warn(format!(
                "{name}: past {MAX_TOKENS} tokens; the rest is not read"
            ));
        }
        let (items, stopped) = parse_file(&toks, &mut b.steps);
        if stopped {
            self.warn(format!(
                "{name}: past the parser's step budget; the rest is not read"
            ));
        }
        for item in items {
            match item {
                Item::Include(f) => self.read_file(source, &f, depth + 1, b),
                Item::Extend(c) => {
                    if self.extends.len() < MAX_CLASSES {
                        self.extends.push(c);
                    }
                }
                Item::Class(c) => {
                    let key = c.name.to_ascii_lowercase();
                    if !self.classes.contains_key(&key) && self.classes.len() >= MAX_CLASSES {
                        self.warn(format!("more than {MAX_CLASSES} classes"));
                        continue;
                    }
                    self.classes.insert(key, c);
                }
            }
        }
    }

    fn apply_extends(&mut self) {
        for e in std::mem::take(&mut self.extends) {
            let Some(c) = self.classes.get_mut(&e.name.to_ascii_lowercase()) else {
                continue;
            };
            for b in e.bases {
                if c.bases.len() < MAX_BASES {
                    c.bases.push(b);
                }
            }
            for k in e.keys {
                if c.keys.len() < MAX_ENTRIES {
                    c.keys.push(k);
                }
            }
            for i in e.inputs {
                if c.inputs.len() < MAX_ENTRIES {
                    c.inputs.push(i);
                }
            }
            for o in e.outputs {
                if c.outputs.len() < MAX_ENTRIES {
                    c.outputs.push(o);
                }
            }
        }
    }
}

/// A class's effective definition: its bases' and its own, flattened.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Flat {
    pub kind: String,
    pub keys: Vec<Rc<KeyDef>>,
    pub inputs: Vec<Rc<IoDef>>,
    pub outputs: Vec<Rc<IoDef>>,
}

impl Flat {
    /// The key `name` (any case).
    pub fn key(&self, name: &str) -> Option<&KeyDef> {
        self.keys
            .iter()
            .find(|k| k.name.eq_ignore_ascii_case(name))
            .map(|k| &**k)
    }

    /// The spawnflags set by default: each flag whose default is 1.
    pub fn default_spawnflags(&self) -> Option<u32> {
        let k = self.key("spawnflags")?;
        Some(
            k.choices
                .iter()
                .filter(|c| c.default)
                .map(|c| crate::value::atoi(&c.value).clamp(0, i64::from(u32::MAX)) as u32)
                .fold(0, |a, b| a | b),
        )
    }

    pub fn has_input(&self, name: &str) -> bool {
        self.inputs
            .iter()
            .any(|i| i.name.eq_ignore_ascii_case(name))
    }

    pub fn has_output(&self, name: &str) -> bool {
        self.outputs
            .iter()
            .any(|o| o.name.eq_ignore_ascii_case(name))
    }
}

/// The most entries (keys, inputs and outputs, each a pointer to an entry shared with the class it comes from) the flattened
/// classes may hold together.
pub const MAX_FLAT_ENTRIES: usize = 1 << 20;
/// The most bytes the flattened classes may hold together: a shared entry counts as a pointer, a flags key a merge had to
/// copy counts its whole text.
pub const MAX_FLAT_BYTES: usize = 64 << 20;
/// The work flattening may do over all classes: a unit for each entry merged and for each 64 bytes of its name.
pub const MAX_FLATTEN_WORK: usize = 1 << 24;

fn hash_of(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// What a key's text takes, in bytes.
fn key_bytes(k: &KeyDef) -> usize {
    k.name.len()
        + k.lower.len()
        + k.default.as_ref().map_or(0, String::len)
        + k.choices.iter().map(|c| c.value.len() + 8).sum::<usize>()
        + std::mem::size_of::<KeyDef>()
}

/// A class being flattened: its entries (shared with the classes they come from) with an index of each by lowercase name
/// (borrowed from those classes), so a merge is one lookup and copies nothing but a pointer.
struct Builder<'a> {
    flat: Flat,
    key_at: HashMap<&'a str, usize>,
    input_at: HashMap<&'a str, usize>,
    output_at: HashMap<&'a str, usize>,
    /// A flags key's choices by the hash of their value, by the key's index (made when a merge first needs it).
    choice_at: HashMap<usize, HashMap<u64, usize>>,
    /// Bytes copied (flags keys a merge changed, and their new choices).
    copied: usize,
    /// The table's work counter, charged before each copy and each hashing.
    work: &'a Cell<usize>,
    /// The bytes the flattened classes held when this one began.
    bytes_before: usize,
    /// Why the merge stopped, when a cap stopped it.
    stopped: Option<&'static str>,
}

impl<'a> Builder<'a> {
    fn new(work: &'a Cell<usize>, bytes_before: usize) -> Builder<'a> {
        Builder {
            flat: Flat::default(),
            key_at: HashMap::new(),
            input_at: HashMap::new(),
            output_at: HashMap::new(),
            choice_at: HashMap::new(),
            copied: 0,
            work,
            bytes_before,
            stopped: None,
        }
    }

    /// Charges `units` of work and `bytes` of copies before they are done; false (and the merge stops) past either cap.
    fn charge(&mut self, units: usize, bytes: usize) -> bool {
        let w = self.work.get().saturating_add(units);
        self.work.set(w);
        if w > MAX_FLATTEN_WORK {
            self.stopped = Some("work");
            return false;
        }
        let b = self
            .bytes_before
            .saturating_add(self.copied)
            .saturating_add(bytes);
        if b > MAX_FLAT_BYTES {
            self.stopped = Some("bytes");
            return false;
        }
        self.copied += bytes;
        true
    }

    fn io(list: &mut Vec<Rc<IoDef>>, at: &mut HashMap<&'a str, usize>, from: &'a [Rc<IoDef>]) {
        for d in from {
            match at.get(d.lower.as_str()) {
                Some(&i) => list[i] = d.clone(),
                None if list.len() < MAX_ENTRIES => {
                    at.insert(d.lower.as_str(), list.len());
                    list.push(d.clone());
                }
                None => {}
            }
        }
    }

    /// Merges `d` into the class; false when a cap stopped it (the class is then not kept).
    fn key(&mut self, d: &'a Rc<KeyDef>) -> bool {
        let Some(&i) = self.key_at.get(d.lower.as_str()) else {
            if self.flat.keys.len() < MAX_ENTRIES {
                self.key_at.insert(d.lower.as_str(), self.flat.keys.len());
                self.flat.keys.push(d.clone());
            }
            return true;
        };
        if self.flat.keys[i].ty != KeyType::Flags || d.ty != KeyType::Flags {
            self.flat.keys[i] = d.clone();
            self.choice_at.remove(&i);
            return true;
        }
        // Flags merge bit by bit: this key becomes the class's own copy, paid for before it is made.
        if Rc::strong_count(&self.flat.keys[i]) > 1 {
            let bytes = key_bytes(&self.flat.keys[i]);
            if !self.charge(1 + bytes / 64, bytes) {
                return false;
            }
        }
        if !self.choice_at.contains_key(&i) {
            let k = &self.flat.keys[i];
            let hashing =
                k.choices.len() + k.choices.iter().map(|c| c.value.len()).sum::<usize>() / 64;
            if !self.charge(hashing, 0) {
                return false;
            }
        }
        let x = Rc::make_mut(&mut self.flat.keys[i]);
        let at = self.choice_at.entry(i).or_insert_with(|| {
            x.choices
                .iter()
                .enumerate()
                .map(|(j, c)| (hash_of(&c.value), j))
                .collect()
        });
        for c in &d.choices {
            // Charged inline (not through `charge`, which needs `self` while `x` and `at` borrow it).
            let w = self.work.get().saturating_add(1 + c.value.len() / 64);
            self.work.set(w);
            if w > MAX_FLATTEN_WORK {
                self.stopped = Some("work");
                return false;
            }
            let found = match at.get(&hash_of(&c.value)) {
                Some(&j) if x.choices[j].value == c.value => Some(j),
                Some(_) => x.choices.iter().position(|y| y.value == c.value),
                None => None,
            };
            match found {
                Some(j) => x.choices[j] = c.clone(),
                None if x.choices.len() < MAX_CHOICES => {
                    let bytes = c.value.len() + 8;
                    if self.bytes_before.saturating_add(self.copied + bytes) > MAX_FLAT_BYTES {
                        self.stopped = Some("bytes");
                        return false;
                    }
                    self.copied += bytes;
                    at.insert(hash_of(&c.value), x.choices.len());
                    x.choices.push(c.clone());
                }
                None => {}
            }
        }
        true
    }
}

/// An FGD's entity classes, flattened when first asked for: the read-only table the spawner reads defaults, types and names
/// from. Each class is flattened once and kept, its entries shared with the classes they come from (a flags key a merge
/// changes is the class's own copy); the flattened classes together hold at most [`MAX_FLAT_ENTRIES`] entries and
/// [`MAX_FLAT_BYTES`] bytes, and
/// flattening does at most [`MAX_FLATTEN_WORK`] units of work, each copy and hashing paid for before it is done. Once a cap
/// is reached a class not yet flattened reads as unknown without being flattened (the coded defaults then apply) and one
/// warning is kept.
#[derive(Debug, Default)]
pub struct EntityDefs {
    classes: HashMap<String, ClassDef>,
    entities: usize,
    cache: RefCell<HashMap<String, Option<Rc<Flat>>>>,
    entries: Cell<usize>,
    bytes: Cell<usize>,
    /// Set once the entries or the bytes passed their caps: later classes are refused without being flattened.
    full: Cell<bool>,
    work: Cell<usize>,
    warnings: RefCell<Vec<String>>,
}

impl EntityDefs {
    /// The table of `fgd`'s classes (nothing is flattened yet).
    pub fn new(fgd: Fgd) -> EntityDefs {
        let entities = fgd
            .classes
            .values()
            .filter(|c| c.kind != "baseclass")
            .count();
        EntityDefs {
            classes: fgd.classes,
            entities,
            ..EntityDefs::default()
        }
    }

    fn warn(&self, w: String) {
        let mut ws = self.warnings.borrow_mut();
        if ws.len() < 256 && !ws.contains(&w) {
            ws.push(w);
        }
    }

    /// The warnings so far: bases cut where a cycle closed or the chain ran too deep, and the caps reached.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }

    fn flatten<'a>(
        &'a self,
        key: &str,
        b: &mut Builder<'a>,
        stack: &mut Vec<String>,
        done: &mut HashSet<String>,
    ) -> bool {
        if stack.iter().any(|s| s == key) {
            self.warn(format!("base cycle at {key}"));
            return true;
        }
        if stack.len() >= MAX_BASE_DEPTH {
            self.warn(format!("bases deeper than {MAX_BASE_DEPTH} at {key}"));
            return true;
        }
        if !done.insert(key.to_string()) {
            return true;
        }
        let Some(c) = self.classes.get(key) else {
            return true;
        };
        stack.push(key.to_string());
        for base in &c.bases {
            if !self.flatten(&base.to_ascii_lowercase(), b, stack, done) {
                return false;
            }
        }
        stack.pop();
        let cost = c
            .keys
            .iter()
            .map(|k| 1 + k.lower.len() / 64 + k.choices.len())
            .chain(
                c.inputs
                    .iter()
                    .chain(&c.outputs)
                    .map(|d| 1 + d.lower.len() / 64),
            )
            .sum::<usize>()
            + 1;
        let work = self.work.get().saturating_add(cost);
        self.work.set(work);
        if work > MAX_FLATTEN_WORK {
            self.warn(format!(
                "flattening passed {MAX_FLATTEN_WORK} units of work; classes not yet read are unknown"
            ));
            return false;
        }
        for k in &c.keys {
            if !b.key(k) {
                self.warn(format!(
                    "flattening passed its {} cap; classes not yet read are unknown",
                    b.stopped.unwrap_or("work")
                ));
                return false;
            }
        }
        Builder::io(&mut b.flat.inputs, &mut b.input_at, &c.inputs);
        Builder::io(&mut b.flat.outputs, &mut b.output_at, &c.outputs);
        true
    }

    /// Reads `file` and its includes from the folder `dir` (the reader's warnings).
    pub fn load_dir(dir: &Path, file: &str) -> (EntityDefs, Vec<String>) {
        let fgd = Fgd::load(
            &DirSource {
                dir: dir.to_path_buf(),
            },
            file,
        );
        let w = fgd.warnings.clone();
        (EntityDefs::new(fgd), w)
    }

    /// The class `name` (any case), flattened (module docs); None for a base class, an unknown one, or past the caps.
    pub fn class(&self, name: &str) -> Option<Rc<Flat>> {
        let key = name.to_ascii_lowercase();
        if let Some(hit) = self.cache.borrow().get(&key) {
            return hit.clone();
        }
        let flat = self.build(&key);
        let mut cache = self.cache.borrow_mut();
        if cache.len() < MAX_CLASSES {
            cache.insert(key, flat.clone());
        }
        flat
    }

    fn build(&self, key: &str) -> Option<Rc<Flat>> {
        let c = self.classes.get(key)?;
        if c.kind == "baseclass" || self.work.get() > MAX_FLATTEN_WORK {
            return None;
        }
        if self.full.get() {
            return None;
        }
        let mut b = Builder::new(&self.work, self.bytes.get());
        b.flat.kind = c.kind.clone();
        if !self.flatten(key, &mut b, &mut Vec::new(), &mut HashSet::new()) {
            if b.stopped == Some("bytes") {
                self.full.set(true);
            }
            return None;
        }
        let copied = b.copied;
        let f = b.flat;
        let size = f.keys.len() + f.inputs.len() + f.outputs.len();
        let total = self.entries.get().saturating_add(size);
        let bytes = self
            .bytes
            .get()
            .saturating_add(size * std::mem::size_of::<Rc<KeyDef>>() + copied);
        if total > MAX_FLAT_ENTRIES || bytes > MAX_FLAT_BYTES {
            self.warn(format!(
                "the flattened classes passed {MAX_FLAT_ENTRIES} entries or {MAX_FLAT_BYTES} bytes; classes not yet read are unknown"
            ));
            self.full.set(true);
            return None;
        }
        self.entries.set(total);
        self.bytes.set(bytes);
        Some(Rc::new(f))
    }

    /// The entries the flattened classes hold together.
    pub fn flat_entries(&self) -> usize {
        self.entries.get()
    }

    /// The bytes the flattened classes hold together (a pointer for each shared entry, and what merges copied).
    pub fn flat_bytes(&self) -> usize {
        self.bytes.get()
    }

    /// The work flattening has done.
    pub fn flatten_work(&self) -> usize {
        self.work.get()
    }

    /// How many entity classes there are.
    pub fn len(&self) -> usize {
        self.entities
    }

    pub fn is_empty(&self) -> bool {
        self.entities == 0
    }

    /// `text` read as the type the class declares for `key` (a string when the class or key is unknown).
    pub fn typed(&self, class: &str, key: &str, text: &str) -> Typed {
        match self.class(class).as_deref().and_then(|c| c.key(key)) {
            Some(k) => k.ty.read(text),
            None => Typed::Str(text.to_string()),
        }
    }
}
