//! Which container a file is: by its magic bytes first, then by its name, then
//! by weak content heuristics. The bytes win over the name, as mesh's own
//! image sniffing trusts magic over labels.

use litegen_mesh::zip;

use crate::Format;

/// How far into a text file the content checks look.
const SNIFF: usize = 64 * 1024;

/// The format of `data`, from its bytes and, when they cannot tell, from
/// `file_name`'s extension. `None` when neither says.
///
/// A zip is judged by its entries alone: a 3MF or a USDZ, or `None` for any
/// other zip (a package of files, which [`crate::Model::read`] unpacks).
pub fn detect_format(data: &[u8], file_name: Option<&str>) -> Option<Format> {
    if let Some(f) = by_magic(data) {
        return Some(f);
    }
    if zip::looks_like_zip(data) {
        return zip_kind(data);
    }
    if let Some(f) = file_name.and_then(by_extension) {
        return Some(f);
    }
    by_content(data)
}

/// The format a file name's extension names, case-insensitively. `.usd`
/// names either USD encoding; when the bytes cannot tell, it reads as
/// [`Format::Usda`].
pub(crate) fn by_extension(name: &str) -> Option<Format> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (_, ext) = base.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "usd" => Some(Format::Usda),
        other => Format::parse(other),
    }
}

fn by_magic(data: &[u8]) -> Option<Format> {
    if data.starts_with(b"glTF") {
        return Some(Format::Glb);
    }
    if data.starts_with(b"Kaydara FBX Binary  \0") {
        return Some(Format::Fbx);
    }
    if data.starts_with(b"PXR-USDC") {
        return Some(Format::Usdc);
    }
    if data.starts_with(b"ply\n") || data.starts_with(b"ply\r\n") {
        return Some(Format::Ply);
    }
    let text = text_start(data);
    if text.starts_with(b"#usda") {
        return Some(Format::Usda);
    }
    if text.starts_with(b"; FBX") {
        return Some(Format::Fbx);
    }
    if contains(&text[..text.len().min(4096)], b"<COLLADA") {
        return Some(Format::Dae);
    }
    // glTF requires `asset`, but a self-contained .gltf can put megabytes of
    // data URI before it, so the whole file is searched.
    if text.starts_with(b"{") && contains(text, b"\"asset\"") {
        return Some(Format::Gltf);
    }
    None
}

fn zip_kind(data: &[u8]) -> Option<Format> {
    let archive = zip::Archive::parse(data).ok()?;
    let names: Vec<&str> = archive.entries.iter().map(|e| e.name.as_str()).collect();
    let three_mf = names.iter().any(|n| n.eq_ignore_ascii_case("3D/3dmodel.model"))
        || (names.iter().any(|n| n.eq_ignore_ascii_case("[Content_Types].xml"))
            && names.iter().any(|n| n.to_ascii_lowercase().ends_with(".model")));
    if three_mf {
        return Some(Format::ThreeMf);
    }
    // USDZ's first entry is its root layer (the USDZ specification).
    let first = names.first()?.to_ascii_lowercase();
    if [".usda", ".usdc", ".usd"].iter().any(|ext| first.ends_with(ext)) {
        return Some(Format::Usdz);
    }
    None
}

fn by_content(data: &[u8]) -> Option<Format> {
    // A binary STL's 80-byte header can say anything, "solid" included, so
    // its size is checked before the ASCII form.
    if data.len() >= 84 {
        let count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as u64;
        if 84 + 50 * count == data.len() as u64 {
            return Some(Format::Stl);
        }
    }
    let text = text_start(data);
    let head = &text[..text.len().min(SNIFF)];
    if head.len() >= 5 && head[..5].eq_ignore_ascii_case(b"solid") && contains(head, b"facet") {
        return Some(Format::Stl);
    }
    // An OBJ: `v` lines near the top, and an `f` line anywhere. Exporters
    // write every vertex before the first face, so the faces of an OBJ of a
    // few thousand vertices start past the first SNIFF bytes.
    let starts = |line: &[u8], tag: u8| {
        let line = trim_start(line);
        line.first() == Some(&tag) && matches!(line.get(1), Some(b' ' | b'\t'))
    };
    let mut lines = head.split(|b| *b == b'\n');
    if !lines.any(|line| starts(line, b'v')) {
        return None;
    }
    text.split(|b| *b == b'\n').any(|line| starts(line, b'f')).then_some(Format::Obj)
}

/// `data` without a UTF-8 byte-order mark or leading whitespace.
fn text_start(data: &[u8]) -> &[u8] {
    trim_start(data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data))
}

fn trim_start(s: &[u8]) -> &[u8] {
    let skip = s.iter().take_while(|b| b.is_ascii_whitespace()).count();
    &s[skip..]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
