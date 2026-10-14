//! USDA — USD's text encoding — parsed into a [`Layer`], plus the small
//! formatting helpers the writer shares with it.
//!
//! # What real files contain, and what this reader therefore accepts
//!
//! `usdcat` output is tidy; hand-edited files, DCC exporters and three.js's
//! `USDZExporter` are not. So the grammar here is the full statement grammar of
//! the Sdf text format as assets use it: `#`, `//` and `/* */` comments; single,
//! double and triple-quoted strings with escapes; `@asset@` and `@@@asset@@@`
//! paths; relative `<../Path>` targets (anchored to the prim that authors
//! them); layer, prim and property metadata in parentheses, including list ops
//! (`prepend`/`append`/`add`/`delete`/`reorder`) on references, payloads,
//! inherits, apiSchemas, variantSets, relationship targets and connections;
//! `over` and `class` prims; `variantSet` blocks; `.timeSamples`, `.connect` and
//! `.default` property suffixes; dictionaries; value blocks (`None`).
//! Unrecognised metadata is parsed and ignored — USD itself round-trips
//! unknown layer metadata — but a token the grammar does not allow is an error
//! with a line number, never a skipped line.
//!
//! # Hostile input
//!
//! The prim hierarchy is walked with an explicit stack, values nest at most
//! [`MAX_VALUE_DEPTH`] deep, every array is charged to a value budget before it
//! grows, and error messages quote at most a short snippet.

use super::usd_stage::{
    absolutize, child_path, Layer, ListOp, PrimSpec, Reference, Specifier, Value, MAX_LIST_OP_ITEMS,
    MAX_PATH_LEN,
};
use super::{MeshError, MeshFormat};

/// Nesting of tuples, lists and dictionaries inside one value. A matrix array
/// is three deep; dictionaries in customData rarely exceed four.
const MAX_VALUE_DEPTH: usize = 32;
/// Numbers (and strings) one layer may decode, across all arrays. The same
/// ceiling `glb.rs` puts on decoded attribute values: more than a 128 MB text
/// file of honest numbers holds.
pub const MAX_VALUES: usize = 48_000_000;
/// Open prims and variant blocks at once.
const MAX_NESTING: usize = 1024;

fn not_usda(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::Usda, detail: detail.into() }
}

/// Whether these bytes start like a USDA layer.
pub fn looks_like_usda(bytes: &[u8]) -> bool {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    b.starts_with(b"#usda ")
}

// ─── lexer ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok<'a> {
    Ident(&'a str),
    Num(&'a str),
    Str(String),
    Asset(String),
    Path(&'a str),
    Punct(u8),
    Eof,
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
    peeked: Option<(Tok<'a>, usize, usize)>,
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || (!c.is_ascii() && c.is_alphabetic())
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || (!c.is_ascii() && c.is_alphanumeric())
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, pos: usize) -> Lexer<'a> {
        Lexer { src, pos, line: 1 + src[..pos].matches('\n').count(), peeked: None }
    }

    fn err(&self, detail: impl Into<String>) -> MeshError {
        MeshError::Malformed(format!("usda line {}: {}", self.line, detail.into()))
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn skip_space(&mut self) -> Result<(), MeshError> {
        loop {
            let r = self.rest();
            let Some(c) = r.chars().next() else { return Ok(()) };
            if c == '\n' {
                self.line += 1;
                self.pos += 1;
            } else if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else if c == '#' || r.starts_with("//") {
                let end = r.find('\n').unwrap_or(r.len());
                self.pos += end;
            } else if let Some(body) = r.strip_prefix("/*") {
                let end = body.find("*/").ok_or_else(|| self.err("an unterminated /* comment"))?;
                self.line += r[..end + 4].matches('\n').count();
                self.pos += end + 4;
            } else {
                return Ok(());
            }
        }
    }

    fn peek(&mut self) -> Result<&Tok<'a>, MeshError> {
        if self.peeked.is_none() {
            let start_line = self.line;
            let t = self.lex()?;
            self.peeked = Some((t, self.pos, start_line));
        }
        Ok(self.peeked.as_ref().map(|p| &p.0).unwrap_or(&Tok::Eof))
    }

    fn next(&mut self) -> Result<Tok<'a>, MeshError> {
        if let Some((t, _, _)) = self.peeked.take() {
            return Ok(t);
        }
        self.lex()
    }

    fn lex(&mut self) -> Result<Tok<'a>, MeshError> {
        self.skip_space()?;
        let r = self.rest();
        let Some(c) = r.chars().next() else { return Ok(Tok::Eof) };
        let start = self.pos;
        // Numbers, including a sign or a leading dot, and the signed infinity
        // spellings USD writes as barewords.
        let bytes = r.as_bytes();
        let starts_number = c.is_ascii_digit()
            || ((c == '-' || c == '+' || c == '.') && bytes.get(1).is_some_and(|b| b.is_ascii_digit() || *b == b'.'));
        if starts_number {
            let mut i = 0;
            if matches!(bytes[0], b'-' | b'+') {
                i += 1;
            }
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                let mut j = i + 1;
                if j < bytes.len() && matches!(bytes[j], b'-' | b'+') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j].is_ascii_digit() {
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    i = j;
                }
            }
            self.pos += i;
            return Ok(Tok::Num(&self.src[start..start + i]));
        }
        if r.starts_with("-inf") || r.starts_with("+inf") {
            self.pos += 4;
            return Ok(Tok::Num(&self.src[start..start + 4]));
        }
        if is_ident_start(c) {
            // Namespaced identifiers (`primvars:st`, `info:id`) keep their
            // colons; a colon not followed by an identifier is punctuation
            // (`0: (1, 2)` in time samples).
            let mut end = 0;
            let mut chars = r.char_indices().peekable();
            while let Some((i, ch)) = chars.next() {
                if is_ident_char(ch) {
                    end = i + ch.len_utf8();
                } else if ch == ':' && chars.peek().is_some_and(|(_, n)| is_ident_start(*n)) {
                    end = i + 1;
                } else {
                    break;
                }
            }
            self.pos += end;
            return Ok(Tok::Ident(&self.src[start..start + end]));
        }
        match c {
            '"' | '\'' => self.string(c),
            '@' => self.asset(),
            '<' => {
                let end = r.find('>').ok_or_else(|| self.err("an unterminated <path>"))?;
                if r[..end].contains('\n') {
                    return Err(self.err("a <path> runs across a line break"));
                }
                self.pos += end + 1;
                Ok(Tok::Path(&self.src[start + 1..start + end]))
            }
            '(' | ')' | '[' | ']' | '{' | '}' | '=' | ',' | ':' | ';' | '.' => {
                self.pos += 1;
                Ok(Tok::Punct(c as u8))
            }
            other => Err(self.err(format!("unexpected character {other:?}"))),
        }
    }

    fn string(&mut self, quote: char) -> Result<Tok<'a>, MeshError> {
        let r = self.rest();
        let triple: String = std::iter::repeat_n(quote, 3).collect();
        let (body_start, terminator) = if r.starts_with(&triple) { (3, triple.as_str()) } else { (1, &r[..1]) };
        let body = &r[body_start..];
        let mut out = String::new();
        let mut chars = body.char_indices();
        loop {
            let Some((i, ch)) = chars.next() else {
                return Err(self.err("an unterminated string"));
            };
            if body[i..].starts_with(terminator) {
                self.line += body[..i].matches('\n').count();
                self.pos += body_start + i + terminator.len();
                return Ok(Tok::Str(out));
            }
            if ch == '\n' && terminator.len() == 1 {
                return Err(self.err("a single-quoted string runs across a line break"));
            }
            if ch == '\\' {
                let Some((_, e)) = chars.next() else { return Err(self.err("a string ends in a backslash")) };
                match e {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'a' => out.push('\u{7}'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'v' => out.push('\u{b}'),
                    '0' => out.push('\0'),
                    'x' => {
                        let hex: String = chars.clone().take(2).map(|(_, c)| c).collect();
                        match u8::from_str_radix(&hex, 16) {
                            Ok(v) if hex.len() == 2 => {
                                chars.next();
                                chars.next();
                                out.push(v as char);
                            }
                            _ => out.push('x'),
                        }
                    }
                    other => out.push(other),
                }
            } else {
                out.push(ch);
            }
        }
    }

    fn asset(&mut self) -> Result<Tok<'a>, MeshError> {
        let r = self.rest();
        let (open, close) = if r.starts_with("@@@") { (3, "@@@") } else { (1, "@") };
        let end = r[open..].find(close).ok_or_else(|| self.err("an unterminated @asset@ path"))?;
        let body = &r[open..open + end];
        if body.contains('\n') {
            return Err(self.err("an @asset@ path runs across a line break"));
        }
        self.pos += open + end + close.len();
        Ok(Tok::Asset(body.to_string()))
    }
}

// ─── types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    Bool,
    Int,
    /// `half`/`float` storage.
    F32,
    /// `double` storage.
    F64,
    Str,
    Asset,
    /// A type the grammar names but the scene never reads (`opaque`,
    /// `pathExpression`, ...): parsed generically.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Type {
    base: Base,
    /// Scalars per element: 3 for a vector3, 16 for a matrix4.
    width: usize,
    array: bool,
    /// Text order is (w, x, y, z); stored as [x, y, z, w].
    quat: bool,
    /// Matrices are written as nested tuples, one per row.
    rows: usize,
}

pub(crate) fn type_of(name: &str) -> Type {
    let (scalar, array) = match name.strip_suffix("[]") {
        Some(s) => (s, true),
        None => (name, false),
    };
    let t = |base: Base, width: usize| Type { base, width, array, quat: false, rows: 0 };
    match scalar {
        "bool" => t(Base::Bool, 1),
        "uchar" | "int" | "uint" | "int64" | "uint64" => t(Base::Int, 1),
        "int2" => t(Base::Int, 2),
        "int3" => t(Base::Int, 3),
        "int4" => t(Base::Int, 4),
        "half" | "float" => t(Base::F32, 1),
        "double" | "timecode" => t(Base::F64, 1),
        "string" | "token" => t(Base::Str, 1),
        "asset" => t(Base::Asset, 1),
        "quath" | "quatf" => Type { quat: true, ..t(Base::F32, 4) },
        "quatd" => Type { quat: true, ..t(Base::F64, 4) },
        "matrix2d" => Type { rows: 2, ..t(Base::F64, 4) },
        "matrix3d" => Type { rows: 3, ..t(Base::F64, 9) },
        "matrix4d" | "frame4d" => Type { rows: 4, ..t(Base::F64, 16) },
        other => {
            // half2..4, float2..4, double2..4 and the role types built on them
            // (point3f, normal3f, vector3f, color3f, color4f, texCoord2f,
            // texCoord3d, ...): the digit is the width, the last letter the
            // precision.
            let digit = other.chars().find(|c| c.is_ascii_digit()).and_then(|c| c.to_digit(10));
            let base = if other.starts_with("double") || (other.ends_with('d') && digit.is_some()) {
                Base::F64
            } else if other.starts_with("half") || other.starts_with("float") || ((other.ends_with('f') || other.ends_with('h')) && digit.is_some()) {
                Base::F32
            } else {
                Base::Other
            };
            match (base, digit) {
                (Base::Other, _) | (_, None) => t(Base::Other, 1),
                (b, Some(d)) if (2..=4).contains(&d) => t(b, d as usize),
                _ => t(Base::Other, 1),
            }
        }
    }
}

// ─── parser ─────────────────────────────────────────────────────────────────

enum Frame {
    /// A prim (or variant) body; properties and child prims go to this spec.
    Body { path: String },
    /// Inside `variantSet "set" = { ... }`, expecting `"variant" { ... }`.
    VariantSet { prim: String, set: String },
}

struct Parser<'a> {
    lx: Lexer<'a>,
    layer: Layer,
    values: usize,
}

/// Parse a USDA layer. `id` is its path inside the package.
pub fn parse(bytes: &[u8], id: &str) -> Result<Layer, MeshError> {
    if !looks_like_usda(bytes) {
        return Err(not_usda("a USDA layer starts with `#usda 1.0`"));
    }
    let text = std::str::from_utf8(bytes).map_err(|e| {
        MeshError::Malformed(format!("usda: the layer is not UTF-8 (invalid byte at offset {})", e.valid_up_to()))
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let header_end = text.find('\n').unwrap_or(text.len());
    let version = text["#usda".len()..header_end].trim();
    if !version.starts_with("1.") {
        return Err(MeshError::Unsupported(format!(
            "usda: text format version {:?}; only 1.x is read",
            super::usd_stage::short(version)
        )));
    }
    let mut p = Parser { lx: Lexer::new(text, header_end), layer: Layer { id: id.to_string(), ..Default::default() }, values: 0 };
    p.layer_metadata()?;
    p.body()?;
    Ok(p.layer)
}

impl<'a> Parser<'a> {
    fn err(&self, detail: impl Into<String>) -> MeshError {
        self.lx.err(detail)
    }

    fn expect(&mut self, c: u8) -> Result<(), MeshError> {
        match self.lx.next()? {
            Tok::Punct(p) if p == c => Ok(()),
            other => Err(self.err(format!("expected '{}', found {}", c as char, describe(&other)))),
        }
    }

    fn eat(&mut self, c: u8) -> Result<bool, MeshError> {
        if *self.lx.peek()? == Tok::Punct(c) {
            self.lx.next()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn charge(&mut self, n: usize) -> Result<(), MeshError> {
        self.values = self.values.saturating_add(n);
        if self.values > MAX_VALUES {
            return Err(MeshError::Unsupported(format!(
                "usda: the layer holds more than {MAX_VALUES} values; more than this converter will hold"
            )));
        }
        Ok(())
    }

    fn ident(&mut self, what: &str) -> Result<&'a str, MeshError> {
        match self.lx.next()? {
            Tok::Ident(s) => Ok(s),
            other => Err(self.err(format!("expected {what}, found {}", describe(&other)))),
        }
    }

    fn string(&mut self, what: &str) -> Result<String, MeshError> {
        match self.lx.next()? {
            Tok::Str(s) => Ok(s),
            other => Err(self.err(format!("expected {what} (a quoted string), found {}", describe(&other)))),
        }
    }

    // ── layer ───────────────────────────────────────────────────────────────

    fn layer_metadata(&mut self) -> Result<(), MeshError> {
        if !self.eat(b'(')? {
            return Ok(());
        }
        loop {
            match self.lx.next()? {
                Tok::Punct(b')') => return Ok(()),
                Tok::Punct(b';') => {}
                Tok::Str(doc) => self.layer.meta.push(("doc".into(), Value::Str(doc))),
                Tok::Ident(key) => {
                    self.expect(b'=')?;
                    if key == "subLayers" {
                        self.sublayers()?;
                    } else {
                        let v = self.generic(0)?;
                        self.layer.meta.push((key.to_string(), v));
                    }
                }
                other => return Err(self.err(format!("unexpected {} in layer metadata", describe(&other)))),
            }
        }
    }

    fn sublayers(&mut self) -> Result<(), MeshError> {
        self.expect(b'[')?;
        loop {
            match self.lx.next()? {
                Tok::Punct(b']') => return Ok(()),
                Tok::Punct(b',') => {}
                Tok::Asset(a) => {
                    let (o, s) = self.layer_offset()?;
                    if self.layer.sublayers.len() >= MAX_LIST_OP_ITEMS {
                        return Err(self.err("too many sublayers"));
                    }
                    self.layer.sublayers.push((a, o, s));
                }
                other => return Err(self.err(format!("expected a sublayer @path@, found {}", describe(&other)))),
            }
        }
    }

    /// An optional `(offset = o; scale = s)` after an asset path.
    fn layer_offset(&mut self) -> Result<(f64, f64), MeshError> {
        let (mut offset, mut scale) = (0.0, 1.0);
        if !self.eat(b'(')? {
            return Ok((offset, scale));
        }
        loop {
            match self.lx.next()? {
                Tok::Punct(b')') => return Ok((offset, scale)),
                Tok::Punct(b';') | Tok::Punct(b',') => {}
                Tok::Ident(k) => {
                    self.expect(b'=')?;
                    let v = self.generic(0)?;
                    match k {
                        "offset" => offset = v.as_f64().ok_or_else(|| self.err("a layer offset is not a number"))?,
                        "scale" => scale = v.as_f64().ok_or_else(|| self.err("a layer scale is not a number"))?,
                        _ => {}
                    }
                }
                other => return Err(self.err(format!("unexpected {} in a layer offset", describe(&other)))),
            }
        }
    }

    // ── namespace ───────────────────────────────────────────────────────────

    fn body(&mut self) -> Result<(), MeshError> {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let tok = self.lx.next()?;
            match (stack.last(), tok) {
                (None, Tok::Eof) => return Ok(()),
                (Some(_), Tok::Eof) => return Err(self.err("the file ends inside a prim; a '}' is missing")),
                (Some(Frame::VariantSet { .. }), Tok::Punct(b'}')) | (Some(Frame::Body { .. }), Tok::Punct(b'}')) => {
                    stack.pop();
                }
                (Some(Frame::VariantSet { prim, set }), Tok::Str(variant)) => {
                    let (prim, set) = (prim.clone(), set.clone());
                    let vpath = format!("{prim}{{{set}={variant}}}");
                    if self.layer.specs.contains_key(&vpath) {
                        return Err(self.err(format!("variant {variant:?} of set {set:?} is defined twice")));
                    }
                    let mut spec = PrimSpec { specifier: Specifier::Over, ..Default::default() };
                    if *self.lx.peek()? == Tok::Punct(b'(') {
                        self.lx.next()?;
                        self.prim_metadata(&mut spec, &prim)?;
                    }
                    self.expect(b'{')?;
                    let owner = self.layer.specs.get_mut(&prim).ok_or_else(|| MeshError::Malformed("usda: a variant set outside its prim".into()))?;
                    match owner.variant_sets.iter_mut().find(|(n, _)| *n == set) {
                        Some((_, vs)) => vs.push(variant),
                        None => owner.variant_sets.push((set, vec![variant])),
                    }
                    self.layer.specs.insert(vpath.clone(), spec);
                    if stack.len() >= MAX_NESTING {
                        return Err(self.err("prims nest deeper than any real hierarchy"));
                    }
                    stack.push(Frame::Body { path: vpath });
                }
                (Some(Frame::VariantSet { .. }), other) => {
                    return Err(self.err(format!("expected a variant name, found {}", describe(&other))));
                }
                (frame, Tok::Ident(kw @ ("def" | "over" | "class"))) => {
                    let parent = match frame {
                        Some(Frame::Body { path }) => Some(path.clone()),
                        _ => None,
                    };
                    let path = self.prim_header(kw, parent.as_deref())?;
                    if stack.len() >= MAX_NESTING {
                        return Err(self.err("prims nest deeper than any real hierarchy"));
                    }
                    stack.push(Frame::Body { path });
                }
                (Some(Frame::Body { path }), Tok::Ident("variantSet")) => {
                    let prim = path.clone();
                    let set = self.string("a variant set name")?;
                    self.expect(b'=')?;
                    self.expect(b'{')?;
                    if stack.len() >= MAX_NESTING {
                        return Err(self.err("variant sets nest deeper than any real hierarchy"));
                    }
                    stack.push(Frame::VariantSet { prim, set });
                }
                (None, Tok::Ident("reorder")) => {
                    // `reorder rootPrims = [...]`: ordering only.
                    self.ident("rootPrims")?;
                    self.expect(b'=')?;
                    self.generic(0)?;
                }
                (Some(Frame::Body { path }), Tok::Ident(first)) => {
                    let path = path.clone();
                    self.property(&path, first)?;
                }
                (Some(Frame::Body { .. }), Tok::Punct(b';')) => {}
                (_, other) => return Err(self.err(format!("unexpected {}", describe(&other)))),
            }
        }
    }

    /// `def Type "name" (metadata) {` — returns the new prim's path.
    fn prim_header(&mut self, kw: &str, parent: Option<&str>) -> Result<String, MeshError> {
        let specifier = match kw {
            "def" => Specifier::Def,
            "over" => Specifier::Over,
            _ => Specifier::Class,
        };
        let mut type_name = String::new();
        let name = match self.lx.next()? {
            Tok::Str(n) => n,
            Tok::Ident(t) => {
                type_name = t.to_string();
                self.string("a prim name")?
            }
            other => return Err(self.err(format!("expected a prim type or name, found {}", describe(&other)))),
        };
        if !valid_prim_name(&name) {
            return Err(self.err(format!("{:?} is not a valid prim name", super::usd_stage::short(&name))));
        }
        let path = child_path(parent.unwrap_or("/"), &name);
        if path.len() > MAX_PATH_LEN {
            return Err(self.err("a prim path is longer than any real hierarchy"));
        }
        if self.layer.specs.contains_key(&path) {
            return Err(self.err(format!("prim {} is defined twice in one layer", super::usd_stage::short(&path))));
        }
        let mut spec = PrimSpec { specifier, type_name, ..Default::default() };
        if *self.lx.peek()? == Tok::Punct(b'(') {
            self.lx.next()?;
            self.prim_metadata(&mut spec, &path)?;
        }
        self.expect(b'{')?;
        match parent {
            Some(pp) => self
                .layer
                .specs
                .get_mut(pp)
                .ok_or_else(|| MeshError::Malformed("usda: a prim outside its parent".into()))?
                .children
                .push(name),
            None => self.layer.root_children.push(name),
        }
        self.layer.specs.insert(path.clone(), spec);
        Ok(path)
    }

    fn prim_metadata(&mut self, spec: &mut PrimSpec, path: &str) -> Result<(), MeshError> {
        let anchor = strip_variants(path);
        loop {
            let (op, key) = match self.lx.next()? {
                Tok::Punct(b')') => return Ok(()),
                Tok::Punct(b';') => continue,
                Tok::Str(_doc) => continue,
                Tok::Ident(k @ ("prepend" | "append" | "add" | "delete" | "reorder")) => (Some(k), self.ident("a metadata key")?),
                Tok::Ident(k) => (None, k),
                other => return Err(self.err(format!("unexpected {} in prim metadata", describe(&other)))),
            };
            self.expect(b'=')?;
            match key {
                "references" | "payload" => {
                    let refs = self.reference_list(&anchor)?;
                    let target = if key == "references" { &mut spec.references } else { &mut spec.payloads };
                    edit(target, op, refs);
                }
                "inherits" | "specializes" => {
                    let paths = self.path_list(&anchor)?;
                    let target = if key == "inherits" { &mut spec.inherits } else { &mut spec.specializes };
                    edit(target, op, paths);
                }
                "apiSchemas" | "variantSets" => {
                    let v = self.generic(0)?;
                    let names = string_list(&v).ok_or_else(|| self.err(format!("{key} is not a list of names")))?;
                    let target = if key == "apiSchemas" { &mut spec.api_schemas } else { &mut spec.variant_set_names };
                    edit(target, op, names);
                }
                "variants" => {
                    let v = self.generic(0)?;
                    if let Value::Dict(d) = v {
                        for (k, sel) in d {
                            if let Some(s) = sel.as_str() {
                                spec.variant_selection.retain(|(n, _)| *n != k);
                                spec.variant_selection.push((k, s.to_string()));
                            }
                        }
                    }
                }
                "active" => spec.active = self.generic(0)?.as_bool(),
                "kind" => spec.kind = self.generic(0)?.as_str().map(str::to_string),
                "customData" => {
                    if let Value::Dict(d) = self.generic(0)? {
                        spec.custom_data = Some(d);
                    }
                }
                _ => {
                    self.generic(0)?;
                }
            }
        }
    }

    /// A property statement inside a prim body; `first` is its first word.
    fn property(&mut self, prim: &str, first: &'a str) -> Result<(), MeshError> {
        let mut word = first;
        let mut op: Option<&str> = None;
        if matches!(word, "prepend" | "append" | "add" | "delete" | "reorder") {
            if word == "reorder" && matches!(self.lx.peek()?, Tok::Ident("nameChildren" | "properties")) {
                // Ordering statements change nothing a converter reads.
                self.lx.next()?;
                self.expect(b'=')?;
                self.generic(0)?;
                return Ok(());
            }
            op = Some(word);
            word = self.ident("a property declaration")?;
        }
        if word == "custom" {
            word = self.ident("a property declaration")?;
        }
        if matches!(word, "uniform" | "config" | "varying") {
            word = self.ident("a property type")?;
        }
        let anchor = strip_variants(prim);
        if word == "rel" {
            let name = self.ident("a relationship name")?.to_string();
            if self.eat(b'.')? {
                // `rel x.default = <..>`, the old spelling of a target.
                self.ident("a relationship suffix")?;
            }
            let mut targets = None;
            if self.eat(b'=')? {
                targets = Some(self.path_list(&anchor)?);
            }
            if *self.lx.peek()? == Tok::Punct(b'(') {
                self.lx.next()?;
                self.skip_property_metadata()?;
            }
            let spec = self.spec_mut(prim)?;
            let rel = spec.rel_mut(&name)?;
            if let Some(t) = targets {
                edit(&mut rel.targets, op, t);
            }
            return Ok(());
        }
        // An attribute: type, optional [], name, optional suffix.
        let mut type_name = word.to_string();
        if *self.lx.peek()? == Tok::Punct(b'[') {
            self.lx.next()?;
            self.expect(b']')?;
            type_name.push_str("[]");
        }
        let name = self.ident("an attribute name")?.to_string();
        let mut suffix = "";
        if self.eat(b'.')? {
            suffix = self.ident("an attribute suffix")?;
        }
        let ty = type_of(&type_name);
        let mut default = None;
        let mut samples = None;
        let mut connections = None;
        if self.eat(b'=')? {
            match suffix {
                "" | "default" => default = Some(self.typed(&ty)?),
                "timeSamples" => samples = Some(self.time_samples(&ty)?),
                "connect" => connections = Some(self.path_list(&anchor)?),
                "spline" => {
                    return Err(MeshError::Unsupported(format!(
                        "usda: attribute {name:?} is animated with a spline, which is not read"
                    )))
                }
                other => return Err(self.err(format!("unknown attribute suffix .{other}"))),
            }
        }
        let mut interpolation = None;
        let mut element_size = None;
        if *self.lx.peek()? == Tok::Punct(b'(') {
            self.lx.next()?;
            loop {
                match self.lx.next()? {
                    Tok::Punct(b')') => break,
                    Tok::Punct(b';') | Tok::Str(_) => {}
                    Tok::Ident(k) => {
                        self.expect(b'=')?;
                        let v = self.generic(0)?;
                        match k {
                            "interpolation" => interpolation = v.as_str().map(str::to_string),
                            "elementSize" => element_size = v.as_i64(),
                            _ => {}
                        }
                    }
                    other => return Err(self.err(format!("unexpected {} in attribute metadata", describe(&other)))),
                }
            }
        }
        let spec = self.spec_mut(prim)?;
        let a = spec.attr_mut(&name)?;
        if a.type_name.is_empty() {
            a.type_name = type_name;
        }
        if default.is_some() {
            a.default = default;
        }
        if samples.is_some() {
            a.samples = samples;
        }
        if let Some(c) = connections {
            edit(&mut a.connections, op, c);
        }
        if interpolation.is_some() {
            a.interpolation = interpolation;
        }
        if element_size.is_some() {
            a.element_size = element_size;
        }
        Ok(())
    }

    fn skip_property_metadata(&mut self) -> Result<(), MeshError> {
        loop {
            match self.lx.next()? {
                Tok::Punct(b')') => return Ok(()),
                Tok::Punct(b';') | Tok::Str(_) => {}
                Tok::Ident(_) => {
                    self.expect(b'=')?;
                    self.generic(0)?;
                }
                other => return Err(self.err(format!("unexpected {} in property metadata", describe(&other)))),
            }
        }
    }

    fn spec_mut(&mut self, path: &str) -> Result<&mut PrimSpec, MeshError> {
        self.layer.specs.get_mut(path).ok_or_else(|| MeshError::Malformed("usda: a property outside its prim".into()))
    }

    // ── values ──────────────────────────────────────────────────────────────

    /// A `<path>` or `[<path>, ...]` (or `None`), absolutised against `anchor`.
    fn path_list(&mut self, anchor: &str) -> Result<Vec<String>, MeshError> {
        let mut out = Vec::new();
        match self.lx.next()? {
            Tok::Ident("None") => {}
            Tok::Path(p) => out.push(self.absolute(anchor, p)?),
            Tok::Punct(b'[') => loop {
                match self.lx.next()? {
                    Tok::Punct(b']') => break,
                    Tok::Punct(b',') => {}
                    Tok::Path(p) => {
                        if out.len() >= MAX_LIST_OP_ITEMS {
                            return Err(self.err("too many targets in one list"));
                        }
                        out.push(self.absolute(anchor, p)?);
                    }
                    other => return Err(self.err(format!("expected a <path>, found {}", describe(&other)))),
                }
            },
            other => return Err(self.err(format!("expected a <path> or a list of them, found {}", describe(&other)))),
        }
        Ok(out)
    }

    fn absolute(&self, anchor: &str, p: &str) -> Result<String, MeshError> {
        let p = p.trim();
        if p.is_empty() {
            return Err(self.err("an empty <> path"));
        }
        absolutize(anchor, p).ok_or_else(|| self.err(format!("path <{}> climbs above the root", super::usd_stage::short(p))))
    }

    /// `@a@</p>`, `</p>`, `@a@`, a list of those, or `None`.
    fn reference_list(&mut self, anchor: &str) -> Result<Vec<Reference>, MeshError> {
        let mut out = Vec::new();
        let list = match self.lx.peek()? {
            Tok::Ident("None") => {
                self.lx.next()?;
                return Ok(out);
            }
            Tok::Punct(b'[') => {
                self.lx.next()?;
                true
            }
            _ => false,
        };
        loop {
            if list {
                match self.lx.peek()? {
                    Tok::Punct(b']') => {
                        self.lx.next()?;
                        return Ok(out);
                    }
                    Tok::Punct(b',') => {
                        self.lx.next()?;
                        continue;
                    }
                    _ => {}
                }
            }
            let mut r = Reference { asset: String::new(), prim: String::new(), offset: 0.0, scale: 1.0 };
            match self.lx.next()? {
                Tok::Asset(a) => {
                    r.asset = a;
                    if let Tok::Path(p) = self.lx.peek()?.clone() {
                        self.lx.next()?;
                        r.prim = self.absolute("/", p)?;
                    }
                }
                Tok::Path(p) => r.prim = self.absolute(anchor, p)?,
                other => return Err(self.err(format!("expected a reference, found {}", describe(&other)))),
            }
            if *self.lx.peek()? == Tok::Punct(b'(') {
                let (o, s) = self.layer_offset()?;
                r.offset = o;
                r.scale = s;
            }
            if out.len() >= MAX_LIST_OP_ITEMS {
                return Err(self.err("too many references in one list"));
            }
            out.push(r);
            if !list {
                return Ok(out);
            }
        }
    }

    fn number(&mut self, tok: Tok<'a>) -> Result<f64, MeshError> {
        match tok {
            Tok::Num(n) => n.parse::<f64>().map_err(|_| self.err(format!("{n:?} is not a number"))),
            Tok::Ident("inf") => Ok(f64::INFINITY),
            Tok::Ident("nan") => Ok(f64::NAN),
            Tok::Ident("true") => Ok(1.0),
            Tok::Ident("false") => Ok(0.0),
            other => Err(self.err(format!("expected a number, found {}", describe(&other)))),
        }
    }

    fn number_f32(&mut self, tok: Tok<'a>) -> Result<f32, MeshError> {
        match tok {
            // Parsed straight to f32: going through f64 double-rounds, and the
            // writer's shortest-repr floats must come back bit for bit.
            Tok::Num(n) => n.parse::<f32>().map_err(|_| self.err(format!("{n:?} is not a number"))),
            other => Ok(self.number(other)? as f32),
        }
    }

    fn integer(&mut self, tok: Tok<'a>) -> Result<i64, MeshError> {
        match tok {
            Tok::Num(n) => n
                .parse::<i64>()
                .map_err(|_| self.err(format!("{:?} is not an integer", super::usd_stage::short(n)))),
            Tok::Ident("true") => Ok(1),
            Tok::Ident("false") => Ok(0),
            other => Err(self.err(format!("expected an integer, found {}", describe(&other)))),
        }
    }

    /// One element of width `w`: a bare number when `w == 1`, else a tuple —
    /// nested one level for matrices.
    fn element_f64(&mut self, ty: &Type, out: &mut Vec<f64>) -> Result<(), MeshError> {
        let tok = self.lx.next()?;
        if ty.width == 1 {
            let v = self.number(tok)?;
            out.push(v);
            return Ok(());
        }
        if tok != Tok::Punct(b'(') {
            return Err(self.err(format!("expected a ({}-tuple, found {}", ty.width, describe(&tok))));
        }
        let start = out.len();
        if ty.rows > 0 {
            for r in 0..ty.rows {
                if r > 0 {
                    self.expect(b',')?;
                }
                self.expect(b'(')?;
                for c in 0..ty.rows {
                    if c > 0 {
                        self.expect(b',')?;
                    }
                    let t = self.lx.next()?;
                    let v = self.number(t)?;
                    out.push(v);
                }
                self.expect(b')')?;
            }
        } else {
            for i in 0..ty.width {
                if i > 0 {
                    self.expect(b',')?;
                }
                let t = self.lx.next()?;
                let v = self.number(t)?;
                out.push(v);
            }
        }
        self.expect(b')')?;
        if ty.quat {
            out[start..].rotate_left(1);
        }
        Ok(())
    }

    fn element_f32(&mut self, ty: &Type, out: &mut Vec<f32>) -> Result<(), MeshError> {
        let tok = self.lx.next()?;
        if ty.width == 1 {
            let v = self.number_f32(tok)?;
            out.push(v);
            return Ok(());
        }
        if tok != Tok::Punct(b'(') {
            return Err(self.err(format!("expected a {}-tuple, found {}", ty.width, describe(&tok))));
        }
        let start = out.len();
        for i in 0..ty.width {
            if i > 0 {
                self.expect(b',')?;
            }
            let t = self.lx.next()?;
            let v = self.number_f32(t)?;
            out.push(v);
        }
        self.expect(b')')?;
        if ty.quat {
            out[start..].rotate_left(1);
        }
        Ok(())
    }

    /// A value of a declared type.
    fn typed(&mut self, ty: &Type) -> Result<Value, MeshError> {
        if *self.lx.peek()? == Tok::Ident("None") {
            self.lx.next()?;
            return Ok(Value::Block);
        }
        if ty.base == Base::Other {
            return self.generic(0);
        }
        if !ty.array {
            return match ty.base {
                Base::Bool => {
                    let t = self.lx.next()?;
                    Ok(Value::Bool(self.number(t)? != 0.0))
                }
                Base::Int if ty.width == 1 => {
                    let t = self.lx.next()?;
                    Ok(Value::Int(self.integer(t)?))
                }
                Base::Int | Base::F32 | Base::F64 => {
                    if ty.width == 1 {
                        let t = self.lx.next()?;
                        return Ok(Value::Float(self.number(t)?));
                    }
                    let mut v = Vec::with_capacity(ty.width);
                    self.element_f64(ty, &mut v)?;
                    Ok(Value::Tuple(v))
                }
                Base::Str => match self.lx.next()? {
                    Tok::Str(s) => Ok(Value::Str(s)),
                    // Tokens are quoted, but a bareword is unambiguous here.
                    Tok::Ident(s) => Ok(Value::Str(s.to_string())),
                    other => Err(self.err(format!("expected a string, found {}", describe(&other)))),
                },
                Base::Asset => match self.lx.next()? {
                    Tok::Asset(a) => Ok(Value::Asset(a)),
                    other => Err(self.err(format!("expected an @asset@ path, found {}", describe(&other)))),
                },
                Base::Other => self.generic(0),
            };
        }
        self.expect(b'[')?;
        macro_rules! array {
            ($vec:ident, $push:expr) => {{
                loop {
                    match self.lx.peek()? {
                        Tok::Punct(b']') => {
                            self.lx.next()?;
                            break;
                        }
                        Tok::Punct(b',') => {
                            self.lx.next()?;
                            continue;
                        }
                        _ => {}
                    }
                    self.charge(ty.width)?;
                    $push;
                }
            }};
        }
        Ok(match ty.base {
            Base::F32 => {
                let mut v: Vec<f32> = Vec::new();
                array!(v, self.element_f32(ty, &mut v)?);
                Value::Floats(v)
            }
            Base::F64 => {
                let mut v: Vec<f64> = Vec::new();
                array!(v, self.element_f64(ty, &mut v)?);
                Value::Doubles(v)
            }
            Base::Int | Base::Bool => {
                let mut v: Vec<i64> = Vec::new();
                if ty.width == 1 {
                    array!(v, {
                        let t = self.lx.next()?;
                        v.push(self.integer(t)?)
                    });
                } else {
                    let mut f: Vec<f64> = Vec::new();
                    array!(f, self.element_f64(ty, &mut f)?);
                    v = f.iter().map(|x| *x as i64).collect();
                }
                Value::Ints(v)
            }
            Base::Str | Base::Asset => {
                let mut v: Vec<String> = Vec::new();
                array!(v, {
                    match self.lx.next()? {
                        Tok::Str(s) | Tok::Asset(s) => v.push(s),
                        Tok::Ident(s) => v.push(s.to_string()),
                        other => return Err(self.err(format!("expected a string, found {}", describe(&other)))),
                    }
                });
                if ty.base == Base::Asset {
                    Value::Assets(v)
                } else {
                    Value::Strs(v)
                }
            }
            Base::Other => unreachable!("handled above"),
        })
    }

    fn time_samples(&mut self, ty: &Type) -> Result<Vec<(f64, Value)>, MeshError> {
        self.expect(b'{')?;
        let mut out: Vec<(f64, Value)> = Vec::new();
        loop {
            let tok = self.lx.next()?;
            match tok {
                Tok::Punct(b'}') => break,
                Tok::Punct(b',') => continue,
                t => {
                    let time = self.number(t)?;
                    if !time.is_finite() {
                        return Err(self.err("a time sample at a non-finite time"));
                    }
                    self.expect(b':')?;
                    let v = self.typed(ty)?;
                    self.charge(1)?;
                    out.push((time, v));
                }
            }
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
    }

    /// A metadata value of no declared type.
    fn generic(&mut self, depth: usize) -> Result<Value, MeshError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(self.err("a value nests deeper than any real file"));
        }
        let tok = self.lx.next()?;
        Ok(match tok {
            Tok::Str(s) => Value::Str(s),
            Tok::Num(n) => {
                if n.contains(['.', 'e', 'E']) || n.ends_with("inf") {
                    Value::Float(self.number(Tok::Num(n))?)
                } else {
                    match n.parse::<i64>() {
                        Ok(i) => Value::Int(i),
                        Err(_) => Value::Float(self.number(Tok::Num(n))?),
                    }
                }
            }
            Tok::Ident("true") => Value::Bool(true),
            Tok::Ident("false") => Value::Bool(false),
            Tok::Ident("None") => Value::Block,
            Tok::Ident("inf") => Value::Float(f64::INFINITY),
            Tok::Ident("nan") => Value::Float(f64::NAN),
            Tok::Ident(s) => Value::Str(s.to_string()),
            Tok::Asset(a) => {
                if *self.lx.peek()? == Tok::Punct(b'(') {
                    self.layer_offset()?;
                }
                Value::Asset(a)
            }
            Tok::Path(p) => Value::Path(p.to_string()),
            Tok::Punct(b'[') => {
                let mut items = Vec::new();
                loop {
                    match self.lx.peek()? {
                        Tok::Punct(b']') => {
                            self.lx.next()?;
                            break;
                        }
                        Tok::Punct(b',') => {
                            self.lx.next()?;
                        }
                        _ => {
                            self.charge(1)?;
                            items.push(self.generic(depth + 1)?);
                        }
                    }
                }
                Value::List(items)
            }
            Tok::Punct(b'(') => {
                let mut items = Vec::new();
                loop {
                    match self.lx.peek()? {
                        Tok::Punct(b')') => {
                            self.lx.next()?;
                            break;
                        }
                        Tok::Punct(b',') => {
                            self.lx.next()?;
                        }
                        _ => {
                            self.charge(1)?;
                            items.push(self.generic(depth + 1)?);
                        }
                    }
                }
                if items.iter().all(|v| v.as_f64().is_some()) {
                    Value::Tuple(items.iter().filter_map(Value::as_f64).collect())
                } else {
                    Value::List(items)
                }
            }
            Tok::Punct(b'{') => Value::Dict(self.dictionary(depth + 1)?),
            other => return Err(self.err(format!("expected a value, found {}", describe(&other)))),
        })
    }

    /// `{ type key = value ... }`, the body after the opening brace.
    fn dictionary(&mut self, depth: usize) -> Result<Vec<(String, Value)>, MeshError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(self.err("a dictionary nests deeper than any real file"));
        }
        let mut out: Vec<(String, Value)> = Vec::new();
        loop {
            let ty_name = match self.lx.next()? {
                Tok::Punct(b'}') => return Ok(out),
                Tok::Punct(b';') | Tok::Punct(b',') => continue,
                Tok::Ident(t) => t.to_string(),
                other => return Err(self.err(format!("expected a dictionary entry type, found {}", describe(&other)))),
            };
            let mut type_name = ty_name;
            if *self.lx.peek()? == Tok::Punct(b'[') {
                self.lx.next()?;
                self.expect(b']')?;
                type_name.push_str("[]");
            }
            let key = match self.lx.next()? {
                Tok::Ident(k) => k.to_string(),
                Tok::Str(k) => k,
                other => return Err(self.err(format!("expected a dictionary key, found {}", describe(&other)))),
            };
            self.expect(b'=')?;
            let v = if type_name == "dictionary" {
                self.expect(b'{')?;
                Value::Dict(self.dictionary(depth + 1)?)
            } else {
                self.typed(&type_of(&type_name))?
            };
            self.charge(1)?;
            out.push((key, v));
        }
    }
}

fn edit<T: Clone + PartialEq>(target: &mut Option<ListOp<T>>, op: Option<&str>, items: Vec<T>) {
    let l = target.get_or_insert_with(ListOp::default);
    match op {
        None => {
            *l = ListOp::explicit(items);
        }
        Some("prepend") => l.prepend = items,
        Some("append") => l.append = items,
        Some("add") => l.add = items,
        Some("delete") => l.delete = items,
        // `reorder` changes order only.
        _ => {}
    }
}

fn string_list(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::Str(s) => Some(vec![s.clone()]),
        Value::Block => Some(Vec::new()),
        Value::List(items) => items.iter().map(|i| i.as_str().map(str::to_string)).collect(),
        _ => None,
    }
}

/// A prim path without its variant selections: what relative paths authored
/// inside a variant are anchored to.
fn strip_variants(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut depth = 0usize;
    for c in path.chars() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                // `/A{v=x}B` continues as `/A/B`.
                if depth == 0 {
                    out.push('/');
                }
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let trimmed = out.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.replace("//", "/")
    }
}

/// USD's prim-name rule: an identifier, UTF-8 letters allowed (24.03+).
pub fn valid_prim_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if is_ident_start(c) => chars.all(is_ident_char),
        _ => false,
    }
}

fn describe(t: &Tok<'_>) -> String {
    match t {
        Tok::Ident(s) => format!("`{}`", super::usd_stage::short(s)),
        Tok::Num(n) => format!("number {}", super::usd_stage::short(n)),
        Tok::Str(s) => format!("string {:?}", super::usd_stage::short(s)),
        Tok::Asset(a) => format!("asset @{}@", super::usd_stage::short(a)),
        Tok::Path(p) => format!("path <{}>", super::usd_stage::short(p)),
        Tok::Punct(c) => format!("'{}'", *c as char),
        Tok::Eof => "the end of the file".into(),
    }
}

// ─── writing helpers ────────────────────────────────────────────────────────

/// A string literal: double-quoted, with every character USD's text format
/// would misread escaped.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The shortest decimal that parses back to exactly `v`.
///
/// Rust's `Display` already is shortest-round-trip, but never switches to an
/// exponent, so `1e-30` would be thirty zeros; exponent form keeps such values
/// short and USD, three.js and JSON all read it.
pub fn fmt_f32(v: f32) -> String {
    let a = v.abs();
    if v == 0.0 || (1e-5..1e9).contains(&a) {
        format!("{v}")
    } else {
        format!("{v:e}")
    }
}

pub fn fmt_f64(v: f64) -> String {
    let a = v.abs();
    if v == 0.0 || (1e-5..1e15).contains(&a) {
        format!("{v}")
    } else {
        format!("{v:e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::usd_stage::Prop;

    fn layer(text: &str) -> Layer {
        parse(text.as_bytes(), "t.usda").unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    #[test]
    fn parses_the_grammar_real_files_use() {
        let l = layer(
            r#"#usda 1.0
(
    "doc string"
    customLayerData = {
        string creator = "x"
        dictionary nested = { int[] a = [1, 2] }
    }
    defaultPrim = "Root"
    metersPerUnit = 0.01
    upAxis = "Z"
    subLayers = [@sub.usda@ (offset = 10; scale = 2), @other.usda@]
)
# line comment
// another
/* block
   comment */
class "_Proto" { custom float protoValue = 1 }
def Xform "Root" (
    kind = "component"
    inherits = </_Proto>
    prepend apiSchemas = ["MaterialBindingAPI"]
    variants = { string look = "red" }
    prepend variantSets = "look"
)
{
    string s1 = "quote\" backslash\\ newline\n tab\t"
    string s2 = 'single "quoted" text'
    string s3 = """multi
line"""
    asset a1 = @textures/my file.png@
    asset a2 = @@@weird@name.png@@@
    uniform token[] tk = ["a", "b"]
    int[] empty = []
    bool flag = true
    float blocked = None
    double3 xformOp:translate.timeSamples = {
        0: (0, 0, 0),
        2.5: (1, 0, 0),
    }
    quatf xformOp:orient = (1, 0, 0, 0)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:orient"]
    rel material:binding = <../Materials/M>
    color3f inputs:c.connect = </Root/S.outputs:rgb>
    matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (5, 6, 7, 1) )
    variantSet "look" = {
        "red" { color3f[] primvars:displayColor = [(1, 0, 0)] }
        "blue" (doc = "b") { def Scope "OnlyBlue" {} }
    }
    def Scope "Child_1" { }
}
over "OverOnly" { def Xform "AddedByOver" { } }
"#,
        );
        assert_eq!(l.meta_str("defaultPrim"), Some("Root"));
        assert_eq!(l.meta_f64("metersPerUnit"), Some(0.01));
        assert_eq!(l.sublayers.len(), 2);
        assert_eq!(l.sublayers[0], ("sub.usda".to_string(), 10.0, 2.0));
        assert_eq!(l.root_children, vec!["_Proto", "Root", "OverOnly"]);
        let root = &l.specs["/Root"];
        assert_eq!(root.type_name, "Xform");
        assert_eq!(root.attr("s1").unwrap().default, Some(Value::Str("quote\" backslash\\ newline\n tab\t".into())));
        assert_eq!(root.attr("s3").unwrap().default, Some(Value::Str("multi\nline".into())));
        assert_eq!(root.attr("a2").unwrap().default, Some(Value::Asset("weird@name.png".into())));
        assert_eq!(root.attr("blocked").unwrap().default, Some(Value::Block));
        let samples = root.attr("xformOp:translate").unwrap().samples.as_ref().unwrap();
        assert_eq!(samples[1].0, 2.5);
        // Text order (w, x, y, z) comes out as [x, y, z, w].
        assert_eq!(root.attr("xformOp:orient").unwrap().default, Some(Value::Tuple(vec![0.0, 0.0, 0.0, 1.0])));
        let m = root.attr("xformOp:transform").unwrap().default.clone().unwrap();
        assert_eq!(m.as_tuple().unwrap()[12..15], [5.0, 6.0, 7.0]);
        match root.prop("material:binding") {
            Some(Prop::Rel(r)) => assert_eq!(r.targets.as_ref().unwrap().explicit.as_deref(), Some(&["/Materials/M".to_string()][..])),
            other => panic!("{other:?}"),
        }
        assert_eq!(root.variant_sets, vec![("look".to_string(), vec!["red".to_string(), "blue".to_string()])]);
        assert!(l.specs.contains_key("/Root{look=red}"));
        assert!(l.specs.contains_key("/Root{look=blue}OnlyBlue"));
        assert_eq!(root.variant_selection, vec![("look".to_string(), "red".to_string())]);
        assert_eq!(l.specs["/OverOnly"].specifier, Specifier::Over);
        assert_eq!(l.specs["/_Proto"].specifier, Specifier::Class);
    }

    #[test]
    fn rejects_what_is_not_usda() {
        assert!(matches!(parse(b"glTF....", "x"), Err(MeshError::NotThisFormat { .. })));
        assert!(matches!(parse(b"#usda 2.0\n", "x"), Err(MeshError::Unsupported(_))));
        for bad in [
            "#usda 1.0\ndef Xform \"A\" {",
            "#usda 1.0\ndef Xform \"1bad\" {}",
            "#usda 1.0\ndef Xform \"A\" {} def Xform \"A\" {}",
            "#usda 1.0\ndef Xform \"A\" { float x = [1, 2 }",
            "#usda 1.0\ndef Xform \"A\" { rel r = <../../..> }",
            "#usda 1.0\ndef Xform \"A\" { string s = \"open }",
        ] {
            assert!(matches!(parse(bad.as_bytes(), "x"), Err(MeshError::Malformed(_))), "{bad}");
        }
    }

    #[test]
    fn floats_round_trip_through_text_exactly() {
        for v in [0.1f32, -0.0, 1.0 / 3.0, 1e-30, 3.4e38, 1.175_494_4e-38, 123456.79, 0.7] {
            let s = fmt_f32(v);
            assert_eq!(s.parse::<f32>().unwrap().to_bits(), v.to_bits(), "{v} -> {s}");
        }
        for v in [0.1f64, 1e-300, 20.99999964237213, -12345.678901234567] {
            let s = fmt_f64(v);
            assert_eq!(s.parse::<f64>().unwrap().to_bits(), v.to_bits(), "{v} -> {s}");
        }
    }

    #[test]
    fn quoted_strings_parse_back_to_themselves() {
        for s in ["plain", "with \"quotes\"", "back\\slash", "new\nline", "tab\t", "ünï", "\u{1}ctl"] {
            let text = format!("#usda 1.0\ndef \"A\" {{ string s = {} }}\n", quote(s));
            let l = layer(&text);
            assert_eq!(l.specs["/A"].attr("s").unwrap().default, Some(Value::Str(s.to_string())), "{text}");
        }
    }

    #[test]
    fn never_panics_on_truncations_or_corruption() {
        let text = br#"#usda 1.0
(
    defaultPrim = "Root"
)
def Xform "Root" (prepend references = @a.usda@</X> (offset = 1))
{
    point3f[] points = [(0, 0, 0), (1, 0, 0)]
    quatf[] r.timeSamples = { 0: [(1, 0, 0, 0)], 1: [(0, 1, 0, 0)] }
    variantSet "v" = { "a" { int x = 1 } }
    dictionary d = { string s = "x" }
}
"#;
        for cut in 0..text.len() {
            let _ = parse(&text[..cut], "x");
        }
        for i in 0..text.len() {
            for b in [b'(', b'"', b'@', b'<', b'{', 0xFF, b'\n'] {
                let mut v = text.to_vec();
                v[i] = b;
                let _ = parse(&v, "x");
            }
        }
    }

    #[test]
    fn deep_nesting_is_refused_not_overflowed() {
        let mut text = String::from("#usda 1.0\n");
        for _ in 0..5000 {
            text.push_str("def \"A\" {\n");
        }
        assert!(parse(text.as_bytes(), "x").is_err());
        let mut v = String::from("#usda 1.0\n(\n x = ");
        for _ in 0..10_000 {
            v.push('[');
        }
        assert!(parse(v.as_bytes(), "x").is_err());
    }
}
