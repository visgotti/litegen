//! XML for the containers that are XML (3MF's model part, Collada).
//!
//! A thin layer over `quick-xml` (already linked into litegen through rust-s3,
//! so this adds no code to the binary) that does three things the codecs would
//! otherwise each get wrong:
//!
//! - **namespaces are resolved**, so a 3MF that binds the materials extension to
//!   the prefix `mat:` instead of the conventional `m:` still reads — matching
//!   on a literal prefix is the classic 3MF reader bug;
//! - **entities are never expanded beyond XML's five and character references**
//!   (quick-xml does no DTD processing), which closes the billion-laughs door;
//! - **work is bounded**: nesting depth and element count are capped, so a
//!   hostile document costs an error rather than the process.
//!
//! Two shapes are offered. [`parse`] builds a small DOM, which is right for
//! Collada (few elements, big text payloads). [`stream`] calls back per event
//! without building anything, which is right for a 3MF mesh of a million
//! `<vertex>` elements that a DOM would spend hundreds of megabytes on.

use super::MeshError;
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

const MAX_DEPTH: usize = 512;
/// Elements a DOM may hold. Streaming callers are bounded by the input size.
const MAX_DOM_ELEMENTS: usize = 2_000_000;

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

fn head(reader: &NsReader<&[u8]>, e: &BytesStart) -> Result<Element, MeshError> {
    let (ns, local) = reader.resolve_element(e.name());
    let mut el = Element {
        ns: resolve(&ns),
        name: String::from_utf8_lossy(local.as_ref()).into_owned(),
        ..Default::default()
    };
    for a in e.attributes() {
        let a = a.map_err(|err| malformed(format!("bad attribute on <{}>: {err}", el.name)))?;
        let key = a.key;
        // Namespace declarations are how prefixes bind, not data; they are
        // recorded apart from the attributes.
        if key.as_ref() == b"xmlns" || key.as_ref().starts_with(b"xmlns:") {
            let prefix = String::from_utf8_lossy(key.as_ref().get(6..).unwrap_or(&[])).into_owned();
            let uri = a
                .unescape_value_with(resolve_xml_entity)
                .map_err(|err| malformed(format!("namespace declaration on <{}> does not unescape: {err}", el.name)))?
                .into_owned();
            el.ns_decls.push((prefix, uri));
            continue;
        }
        let (ans, alocal) = reader.resolve_attribute(key);
        // XML's five entities and character references, nothing else: quick-xml
        // 0.32's plain `unescape_value` resolves none of them.
        let value = a
            .unescape_value_with(resolve_xml_entity)
            .map_err(|err| malformed(format!("attribute on <{}> does not unescape: {err}", el.name)))?
            .into_owned();
        el.attrs.push(Attr {
            ns: resolve(&ans),
            name: String::from_utf8_lossy(alocal.as_ref()).into_owned(),
            value,
        });
    }
    Ok(el)
}

/// Walk a document, calling `f` for every element start (with its attributes,
/// but no children), end and text run. `f` may stop the walk early by
/// returning `Ok(false)`.
pub fn stream(bytes: &[u8], mut f: impl FnMut(XmlEvent<'_>) -> Result<bool, MeshError>) -> Result<(), MeshError> {
    let text = std::str::from_utf8(bytes).map_err(|e| malformed(format!("document is not UTF-8 ({e})")))?;
    let mut reader = NsReader::from_str(text);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = true;
    let mut depth = 0usize;
    let mut stack: Vec<String> = Vec::new();
    let mut seen_root = false;
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| malformed(format!("parse error near byte {}: {e}", reader.buffer_position())))?;
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
            Event::Text(t) => {
                let s = t.unescape_with(resolve_xml_entity).map_err(|e| malformed(format!("text does not unescape: {e}")))?;
                if !s.is_empty() && !f(XmlEvent::Text(&s))? {
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
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut count = 0usize;
    stream(bytes, |ev| {
        match ev {
            XmlEvent::Start(e) => {
                count += 1;
                if count > MAX_DOM_ELEMENTS {
                    return Err(malformed(format!("more than {MAX_DOM_ELEMENTS} elements")));
                }
                if root.is_some() && stack.is_empty() {
                    return Err(malformed("more than one root element"));
                }
                stack.push(e.clone());
            }
            XmlEvent::End(_) => {
                let done = stack.pop().ok_or_else(|| malformed("unbalanced end tag"))?;
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
            // XML 1.0 cannot carry most control characters at all, even as
            // references; drop them rather than emit an unparseable file.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
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
}
