//! XML for the containers that are XML (3MF's model part, Collada).
//!
//! A thin layer over `quick-xml` (0.41 or later: earlier versions stored every
//! namespace declaration on a tag before handing it over, RUSTSEC-2026-0195)
//! that does three things the codecs would otherwise each get wrong:
//!
//! - **namespaces are resolved**, so a 3MF that binds the materials extension to
//!   the prefix `mat:` instead of the conventional `m:` still reads — matching
//!   on a literal prefix is the classic 3MF reader bug;
//! - **entities are never expanded beyond XML's five and character references**
//!   (quick-xml does no DTD processing), which closes the billion-laughs door;
//! - **work is bounded**: nesting depth, element count, attributes and
//!   namespace declarations per element are capped, so a hostile document
//!   costs an error rather than the process.
//!
//! Two shapes are offered. [`parse`] builds a small DOM, which is right for
//! Collada (few elements, big text payloads). [`stream`] calls back per event
//! without building anything, which is right for a 3MF mesh of a million
//! `<vertex>` elements that a DOM would spend hundreds of megabytes on.

use super::MeshError;
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use quick_xml::XmlVersion;

const MAX_DEPTH: usize = 512;
/// Elements and attributes a DOM may hold together. Streaming callers are
/// bounded by the input size.
const MAX_DOM_ELEMENTS: usize = 2_000_000;
/// Attributes on one element. Collada and 3MF elements carry a handful;
/// 40k on one tag took 1.5 s through quick-xml's pairwise duplicate check.
const MAX_ATTRIBUTES: usize = 1024;
/// Namespace declarations on one element. Collada declares one, a 3MF model
/// part one per extension it uses (a dozen at most). The reader stores each
/// before handing the element over, so this bounds that storage up front.
const MAX_NAMESPACE_DECLARATIONS: usize = 128;

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("xml: {}", detail.into()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    /// Namespace URI; `None` for an unprefixed attribute (which, per the XML
    /// namespaces spec, is in NO namespace — not the element's default one).
    pub ns: Option<String>,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// Namespace URI of the element, after resolving its prefix (or the
    /// default namespace in scope).
    pub ns: Option<String>,
    /// Local name, prefix stripped.
    pub name: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Element>,
    /// All character data directly inside this element, concatenated.
    pub text: String,
    /// Namespace declarations made ON this element, as `(prefix, uri)`; the
    /// default namespace has the prefix `""`. Element and attribute names are
    /// already resolved through these — they are kept for the attribute
    /// VALUES that name namespaces by prefix (3MF's `requiredextensions`),
    /// which no parser resolves on its own.
    pub ns_decls: Vec<(String, String)>,
}

impl Element {
    /// An attribute in no namespace (the ordinary kind).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.ns.is_none() && a.name == name).map(|a| a.value.as_str())
    }

    /// An attribute in a specific namespace.
    pub fn attr_ns(&self, ns: &str, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.ns.as_deref() == Some(ns) && a.name == name)
            .map(|a| a.value.as_str())
    }

    /// First child with this local name, any namespace.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Depth-first search for every descendant with this local name.
    pub fn descendants_named<'a>(&'a self, name: &'a str) -> Vec<&'a Element> {
        let mut out = Vec::new();
        let mut stack: Vec<&Element> = self.children.iter().rev().collect();
        while let Some(e) = stack.pop() {
            if e.name == name {
                out.push(e);
            }
            stack.extend(e.children.iter().rev());
        }
        out
    }

    pub fn is(&self, ns: &str, name: &str) -> bool {
        self.name == name && self.ns.as_deref() == Some(ns)
    }
}

/// One parse event, with names already resolved.
pub enum XmlEvent<'e> {
    Start(&'e Element),
    End(&'e str),
    Text(&'e str),
}

fn resolve(r: &ResolveResult) -> Option<String> {
    match r {
        ResolveResult::Bound(ns) => Some(String::from_utf8_lossy(ns.as_ref()).into_owned()),
        _ => None,
    }
}

/// An attribute's value as XML reads it (XML 1.0 §3.3.3): XML's five entities
/// and character references resolved, nothing else, and a literal tab or line
/// break read as a space. The five never nest, so one level of expansion is
/// all there is.
fn attr_value(a: &Attribute) -> Result<String, quick_xml::Error> {
    a.normalized_value_with(XmlVersion::Implicit1_0, 1, resolve_xml_entity).map(|v| v.into_owned())
}

/// An entity or character reference in text, by the same rule: no DTD is
/// read, so no entity but XML's five is declared.
fn reference(r: &BytesRef) -> Result<String, MeshError> {
    if let Some(c) = r.resolve_char_ref().map_err(|e| malformed(format!("text does not unescape: {e}")))? {
        return Ok(c.to_string());
    }
    let name = r.decode().map_err(|e| malformed(format!("text does not decode: {e}")))?;
    resolve_xml_entity(&name)
        .map(str::to_string)
        .ok_or_else(|| malformed(format!("text does not unescape: the entity &{name}; is not declared")))
}

fn head(reader: &NsReader<&[u8]>, e: &BytesStart) -> Result<Element, MeshError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    let mut el = Element {
        ns: resolve(&ns),
        name: String::from_utf8_lossy(local.as_ref()).into_owned(),
        ..Default::default()
    };
    // quick-xml's own duplicate check compares every attribute with every
    // earlier one; a set does it in one pass.
    let mut seen: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    for a in e.attributes().with_checks(false) {
        let a = a.map_err(|err| malformed(format!("bad attribute on <{}>: {err}", el.name)))?;
        let key = a.key;
        if seen.len() >= MAX_ATTRIBUTES {
            return Err(MeshError::Unsupported(format!("xml: <{}> has more than {MAX_ATTRIBUTES} attributes", el.name)));
        }
        if !seen.insert(key.as_ref().to_vec()) {
            return Err(malformed(format!(
                "<{}> repeats the attribute {}",
                el.name,
                String::from_utf8_lossy(key.as_ref())
            )));
        }
        // Namespace declarations are how prefixes bind, not data; they are
        // recorded apart from the attributes.
        if key.as_ref() == b"xmlns" || key.as_ref().starts_with(b"xmlns:") {
            let prefix = String::from_utf8_lossy(key.as_ref().get(6..).unwrap_or(&[])).into_owned();
            let uri = attr_value(&a)
                .map_err(|err| malformed(format!("namespace declaration on <{}> does not unescape: {err}", el.name)))?;
            el.ns_decls.push((prefix, uri));
            continue;
        }
        let (ans, alocal) = reader.resolver().resolve_attribute(key);
        let value = attr_value(&a).map_err(|err| malformed(format!("attribute on <{}> does not unescape: {err}", el.name)))?;
        el.attrs.push(Attr {
            ns: resolve(&ans),
            name: String::from_utf8_lossy(alocal.as_ref()).into_owned(),
            value,
        });
    }
    Ok(el)
}

/// Refuse a DOCTYPE that carries markup before its first `>`, in one linear
/// pass, before quick-xml sees the document.
///
/// quick-xml 0.32 treated any `<!D` / `<!d` as a DOCTYPE and found its end by
/// recounting the `<`/`>` balance from the DOCTYPE's start at every later `>`,
/// which is quadratic in the document once an internal subset (`[<!ENTITY …>]`)
/// keeps the balance open: a 10 KB DEFLATEd `.rels` part cost days of CPU. No
/// format read here needs a DTD (3MF forbids one, Collada is XSD-based), so
/// whatever later versions do, only the plain `<!DOCTYPE name …>` form, which
/// closes at its first `>`, gets through.
fn refuse_dtd_markup(b: &[u8]) -> Result<(), MeshError> {
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'<' && b[i + 1] == b'!' && matches!(b[i + 2], b'D' | b'd') {
            let mut j = i + 3;
            loop {
                match b.get(j) {
                    Some(b'>') => break,
                    Some(b'<') | None => {
                        return Err(malformed(
                            "a DOCTYPE with an internal subset, or one that never closes, is not supported",
                        ))
                    }
                    Some(_) => j += 1,
                }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// Walk a document, calling `f` for every element start (with its attributes,
/// but no children), end and text run. `f` may stop the walk early by
/// returning `Ok(false)`.
pub fn stream(bytes: &[u8], mut f: impl FnMut(XmlEvent<'_>) -> Result<bool, MeshError>) -> Result<(), MeshError> {
    let text = std::str::from_utf8(bytes).map_err(|e| malformed(format!("document is not UTF-8 ({e})")))?;
    refuse_dtd_markup(text.as_bytes())?;
    let mut reader = NsReader::from_str(text);
    // Text is passed untrimmed: a comment, processing instruction or CDATA
    // section splits an element's character data without ending it (XML 1.0
    // §2.4), and trimming each piece fused `1 0 <!-- c --> 0 1` into
    // `1 00 1`. Consumers trim an element's whole text once ([`parse`]).
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = true;
    // The reader stores an element's namespace declarations before handing
    // the element over; past this many it refuses the element instead.
    reader.resolver_mut().set_max_declarations_per_element(MAX_NAMESPACE_DECLARATIONS);
    let mut depth = 0usize;
    let mut stack: Vec<String> = Vec::new();
    let mut seen_root = false;
    // Whether the last event split a text run; whitespace between elements
    // is skipped as before, whitespace inside a split run is text.
    let mut split = false;
    // The character data between two pieces of markup arrives in pieces:
    // plain text, and each entity or character reference on its own. They are
    // joined back into one run and handed over as one text, so `&lt; &gt;`
    // reads `< >`. `blank`: the run is only whitespace as written (a
    // reference never is).
    let mut run = String::new();
    let mut in_run = false;
    let mut blank = true;
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| malformed(format!("parse error near byte {}: {e}", reader.buffer_position())))?;
        match &ev {
            Event::Text(t) => {
                blank &= t.iter().all(|c| c.is_ascii_whitespace());
                run.push_str(&t.decode().map_err(|e| malformed(format!("text does not decode: {e}")))?);
                in_run = true;
                continue;
            }
            Event::GeneralRef(r) => {
                blank = false;
                run.push_str(&reference(r)?);
                in_run = true;
                continue;
            }
            _ => {}
        }
        if std::mem::take(&mut in_run) {
            let text = std::mem::take(&mut run);
            let was_blank = std::mem::replace(&mut blank, true);
            // Whitespace between elements is skipped; after a comment,
            // processing instruction or CDATA section it continues a run.
            if (split || !was_blank) && !text.is_empty() && !f(XmlEvent::Text(&text))? {
                return Ok(());
            }
        }
        split = matches!(ev, Event::Comment(_) | Event::PI(_) | Event::CData(_));
        match ev {
            Event::Start(ref e) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(malformed(format!("nesting deeper than {MAX_DEPTH}")));
                }
                let el = head(&reader, e)?;
                seen_root = true;
                stack.push(el.name.clone());
                if !f(XmlEvent::Start(&el))? {
                    return Ok(());
                }
            }
            Event::Empty(ref e) => {
                let el = head(&reader, e)?;
                seen_root = true;
                if !f(XmlEvent::Start(&el))? || !f(XmlEvent::End(&el.name))? {
                    return Ok(());
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                let name = stack.pop().unwrap_or_default();
                if !f(XmlEvent::End(&name))? {
                    return Ok(());
                }
            }
            Event::CData(t) => {
                let s = String::from_utf8_lossy(t.as_ref()).into_owned();
                if !f(XmlEvent::Text(&s))? {
                    return Ok(());
                }
            }
            Event::Eof => break,
            // Declarations, comments, processing instructions and DOCTYPE carry
            // nothing a mesh needs; DOCTYPE in particular is never expanded.
            _ => {}
        }
    }
    if !seen_root {
        return Err(malformed("document has no root element"));
    }
    if !stack.is_empty() {
        return Err(malformed(format!("document ends inside <{}>", stack.last().map_or("", |s| s.as_str()))));
    }
    Ok(())
}

/// Parse a whole document into a DOM and return its root element.
pub fn parse(bytes: &[u8]) -> Result<Element, MeshError> {
    parse_capped(bytes, MAX_DOM_ELEMENTS)
}

/// [`parse`], holding at most `max_nodes` elements and attributes together:
/// a DOM node costs a few hundred bytes, so a part that only ever holds a
/// few (a package's relationships) gets a cap of its own — 2M `<a/>` in an
/// 8 KB deflated `.rels` read at 461 MB.
pub fn parse_capped(bytes: &[u8], max_nodes: usize) -> Result<Element, MeshError> {
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut count = 0usize;
    stream(bytes, |ev| {
        match ev {
            XmlEvent::Start(e) => {
                count = count.saturating_add(1 + e.attrs.len());
                if count > max_nodes {
                    return Err(MeshError::Unsupported(format!("xml: more than {max_nodes} elements and attributes")));
                }
                if root.is_some() && stack.is_empty() {
                    return Err(malformed("more than one root element"));
                }
                stack.push(e.clone());
            }
            XmlEvent::End(_) => {
                let mut done = stack.pop().ok_or_else(|| malformed("unbalanced end tag"))?;
                let xml_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r');
                if done.text.starts_with(xml_space) || done.text.ends_with(xml_space) {
                    done.text = done.text.trim_matches(xml_space).to_string();
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => root = Some(done),
                }
            }
            XmlEvent::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(t);
                }
            }
        }
        Ok(true)
    })?;
    root.ok_or_else(|| malformed("document has no root element"))
}

/// Escape text for an attribute value or element content.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 cannot carry most control characters, nor U+FFFE and
            // U+FFFF (the `Char` production, §2.2), even as references; drop
            // them rather than emit an unparseable file.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// Parse a whitespace-separated list of numbers (Collada arrays, 3MF matrices).
pub fn floats(text: &str) -> Result<Vec<f32>, MeshError> {
    text.split_ascii_whitespace()
        .map(|t| {
            t.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| malformed(format!("{:?} is not a finite number", &t[..t.len().min(40)])))
        })
        .collect()
}

pub fn ints(text: &str) -> Result<Vec<i64>, MeshError> {
    text.split_ascii_whitespace()
        .map(|t| t.parse::<i64>().map_err(|_| malformed(format!("{:?} is not an integer", &t[..t.len().min(40)]))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-12 review, finding 26.7: XML 1.0's `Char` production (§2.2)
    /// excludes U+FFFE and U+FFFF; escaping drops them with the C0 controls.
    #[test]
    fn escaping_drops_what_xml_cannot_carry() {
        assert_eq!(escape("a\u{FFFE}b\u{FFFF}c\u{1}d\te"), "abcd\te");
    }

    /// 2026-10-12 review, finding 26.1: an element's text is all of its
    /// character data, comments, processing instructions and CDATA sections
    /// only splitting it (XML 1.0 §2.4, §2.5, §2.7). Each piece used to be
    /// trimmed and the pieces fused: `1 0 <!-- tint --> 0 1` read as
    /// `1 00 1`.
    #[test]
    fn text_split_by_a_comment_pi_or_cdata_is_one_text() {
        let words = |doc: &[u8]| parse(doc).unwrap().text.split_ascii_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(words(b"<c>1 0 <!-- tint --> 0 1</c>"), ["1", "0", "0", "1"]);
        assert_eq!(words(b"<c>1 0<?pi x?> 0 1</c>"), ["1", "0", "0", "1"]);
        assert_eq!(words(b"<c>1 <![CDATA[2]]> 3</c>"), ["1", "2", "3"]);
        // No whitespace on either side: XML's text is the pieces joined.
        assert_eq!(parse(b"<c>tex<!-- x -->ture.png</c>").unwrap().text, "texture.png");
        assert_eq!(parse(b"<c>\n  1 2\n</c>").unwrap().text, "1 2");
        assert_eq!(parse(b"<c> <d/> </c>").unwrap().text, "");
    }

    #[test]
    fn resolves_namespaces_whatever_the_prefix() {
        let doc = br#"<?xml version="1.0"?>
            <model xmlns="urn:core" xmlns:mat="urn:materials">
              <mat:texture2d id="1" path="/t.png"/>
              <resources><object id="2" mat:pid="1"/></resources>
            </model>"#;
        let root = parse(doc).unwrap();
        assert!(root.is("urn:core", "model"));
        let tex = root.child("texture2d").unwrap();
        assert_eq!(tex.ns.as_deref(), Some("urn:materials"));
        assert_eq!(tex.attr("path"), Some("/t.png"));
        let obj = &root.child("resources").unwrap().children[0];
        assert_eq!(obj.attr("pid"), None, "a prefixed attribute is not the plain one");
        assert_eq!(obj.attr_ns("urn:materials", "pid"), Some("1"));
    }

    #[test]
    fn records_namespace_declarations_apart_from_attributes() {
        let root = parse(br#"<model xmlns="urn:core" xmlns:m="urn:mat" requiredextensions="m"/>"#).unwrap();
        assert_eq!(
            root.ns_decls,
            vec![("".to_string(), "urn:core".to_string()), ("m".to_string(), "urn:mat".to_string())]
        );
        assert_eq!(root.attrs.len(), 1, "declarations are not attributes: {:?}", root.attrs);
        assert_eq!(root.attr("requiredextensions"), Some("m"));
    }

    #[test]
    fn a_byte_order_mark_before_the_declaration_is_accepted() {
        // Notepad-era tools still write one, and the 3MF conformance suite's
        // MUSTPASS files carry it.
        let root = parse(b"\xEF\xBB\xBF<?xml version=\"1.0\" encoding=\"UTF-8\"?><a v=\"1\"/>").unwrap();
        assert_eq!(root.attr("v"), Some("1"));
    }

    #[test]
    fn unescapes_text_and_attributes() {
        let root = parse(br#"<a v="x &amp; y &#65;">1 &lt; 2</a>"#).unwrap();
        assert_eq!(root.attr("v"), Some("x & y A"));
        assert_eq!(root.text, "1 < 2");
    }

    #[test]
    fn refuses_custom_entities_rather_than_expanding_them() {
        let doc = br#"<!DOCTYPE a [<!ENTITY lol "lol">]><a>&lol;</a>"#;
        assert!(parse(doc).is_err());
    }

    #[test]
    fn an_unbalanced_doctype_is_refused_in_linear_time() {
        // quick-xml rescans a DOCTYPE from its start at every `>`; this shape
        // used to take seconds at 80 KB and days at 10 MB.
        let mut doc = b"<!DOCTYPE r [<]>".to_vec();
        for _ in 0..200_000 {
            doc.extend_from_slice(b"<>");
        }
        let started = std::time::Instant::now();
        let e = parse(&doc).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "took {:?}", started.elapsed());
        assert!(e.to_string().contains("DOCTYPE"), "{e}");
        // Lowercase and post-root spellings reach the same quick-xml path.
        assert!(parse(b"<a/><!doctype x [<y>]>").is_err());
    }

    #[test]
    fn a_plain_doctype_still_parses() {
        let root = parse(br#"<?xml version="1.0"?><!DOCTYPE COLLADA><COLLADA v="1"/>"#).unwrap();
        assert_eq!(root.name, "COLLADA");
    }

    #[test]
    fn refuses_mismatched_and_unterminated_documents() {
        assert!(parse(b"<a><b></a>").is_err());
        assert!(parse(b"<a><b>").is_err());
        assert!(parse(b"").is_err());
        assert!(parse(b"<a/><b/>").is_err());
    }

    #[test]
    fn bounds_nesting_depth() {
        let deep = "<a>".repeat(MAX_DEPTH + 1) + &"</a>".repeat(MAX_DEPTH + 1);
        assert!(parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn escape_round_trips() {
        let s = "a<b>&\"c'\u{1}d";
        let doc = format!("<x v=\"{}\">{}</x>", escape(s), escape(s));
        let root = parse(doc.as_bytes()).unwrap();
        assert_eq!(root.attr("v"), Some("a<b>&\"c'd"));
        assert_eq!(root.text, "a<b>&\"c'd");
    }

    #[test]
    fn number_lists_parse_and_refuse_garbage() {
        assert_eq!(floats(" 1 2.5\n-3e1 ").unwrap(), vec![1.0, 2.5, -30.0]);
        assert!(floats("1 nan").is_err());
        assert_eq!(ints("3 -1 0").unwrap(), vec![3, -1, 0]);
        assert!(ints("1.5").is_err());
    }

    /// 2026-10-12 review, finding 13: quick-xml's duplicate-attribute check
    /// is pairwise, and nothing capped attributes per element or counted them
    /// in the DOM: 40k attributes on one tag took 1.5 s, ~13M in a 128 MB
    /// Collada document an estimated 45 h.
    #[test]
    fn attributes_are_capped_per_element_and_duplicates_found_in_one_pass() {
        let many: String = (0..40_000).map(|i| format!(" a{i}=\"{i}\"")).collect();
        let started = std::time::Instant::now();
        let e = parse(format!("<r{many}/>").as_bytes()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("attributes")), "{e:?}");
        assert!(started.elapsed().as_millis() < 500, "{:?}", started.elapsed());
        assert!(matches!(parse(b"<r a=\"1\" b=\"2\" a=\"3\"/>"), Err(MeshError::Malformed(_))), "a repeated attribute is still refused");
        let fine: String = (0..MAX_ATTRIBUTES).map(|i| format!(" a{i}=\"{i}\"")).collect();
        assert_eq!(parse(format!("<r{fine}/>").as_bytes()).unwrap().attrs.len(), MAX_ATTRIBUTES);
        // A small part with a cap of its own.
        let rels = format!("<Relationships>{}</Relationships>", "<a/>".repeat(20_000));
        assert!(parse_capped(rels.as_bytes(), 10_000).is_err());
        assert!(parse(rels.as_bytes()).is_ok());
    }

    fn declarations(n: usize) -> String {
        (0..n).map(|i| format!(" xmlns:p{i}=\"urn:{i}\"")).collect()
    }

    /// RUSTSEC-2026-0195: before 0.41, quick-xml's namespace reader stored
    /// every declaration on a start tag before returning the tag, about three
    /// times the tag's size in heap, so `head`'s attribute cap came too late
    /// (an upload inflating to a 1 GiB start tag cost about 3 GiB). Since 0.41
    /// the reader refuses a tag past its limit before storing anything. This
    /// pins that guard against a downgrade.
    #[test]
    fn the_reader_bounds_namespace_declarations_before_storing_them() {
        let doc = format!("<r{}/>", declarations(300));
        let mut reader = NsReader::from_str(&doc);
        assert!(
            reader.read_event().is_err(),
            "quick-xml >= 0.41 refuses a tag past its declaration limit before storing it"
        );
    }

    /// The limit `stream` sets is its own, whatever quick-xml's default.
    #[test]
    fn a_tag_with_too_many_namespace_declarations_is_refused() {
        let at_limit = format!("<r{}/>", declarations(MAX_NAMESPACE_DECLARATIONS));
        assert_eq!(parse(at_limit.as_bytes()).unwrap().ns_decls.len(), MAX_NAMESPACE_DECLARATIONS);
        let past = format!("<r{}/>", declarations(MAX_NAMESPACE_DECLARATIONS + 1));
        assert!(parse(past.as_bytes()).is_err());
        let nested = format!("<r><s{}/></r>", declarations(MAX_NAMESPACE_DECLARATIONS + 1));
        assert!(parse(nested.as_bytes()).is_err());
    }

    /// Text around entity and character references is one text: `&lt; &gt;`
    /// reads `< >`, not `<>`. Anything but XML's five entities is refused.
    #[test]
    fn entity_references_join_the_text_around_them() {
        assert_eq!(parse(b"<c>x &amp; y</c>").unwrap().text, "x & y");
        assert_eq!(parse(b"<c>&lt; &gt;</c>").unwrap().text, "< >");
        assert_eq!(parse(b"<c>&#65;&#x42; &quot;&apos;</c>").unwrap().text, "AB \"'");
        assert_eq!(parse(b"<c>1 &#32;<!-- x --> 2</c>").unwrap().text, "1   2");
        assert!(parse(b"<c>&lol;</c>").is_err());
        assert!(parse(b"<c>&#0;</c>").is_err());
        // In a streamed run, the pieces arrive as one text event.
        let mut texts = Vec::new();
        stream(b"<c>a &lt; b</c>", |ev| {
            if let XmlEvent::Text(t) = ev {
                texts.push(t.to_string());
            }
            Ok(true)
        })
        .unwrap();
        assert_eq!(texts, ["a < b"]);
    }

    /// Attribute values are normalized as XML 1.0 §3.3.3 says: a literal tab,
    /// carriage return or line feed reads as a space, while a character
    /// reference keeps what it names.
    #[test]
    fn attribute_values_are_normalized_as_xml_says() {
        let root = parse(b"<a v=\"x\ty\nz\" w=\"1&#9;2\" e=\"x &amp; y\"/>").unwrap();
        assert_eq!(root.attr("v"), Some("x y z"));
        assert_eq!(root.attr("w"), Some("1\t2"));
        assert_eq!(root.attr("e"), Some("x & y"));
    }
}
