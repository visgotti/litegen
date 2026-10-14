//! Files that travel together.
//!
//! Several containers are not one file. A `.gltf` names a `.bin` and its
//! images; an OBJ names an `.mtl`, which names textures; a Collada document
//! names its images. Reading one of those means resolving names the file chose,
//! and writing one means producing a primary file plus the sidecars its
//! references expect to find beside it.
//!
//! This module never touches a filesystem or a network. A reader asks a
//! [`Sidecars`] implementation for a path and gets bytes or nothing; what backs
//! that — a directory in a test, a vendor's bundle in the proxy — is the
//! caller's business. That is also the security boundary: the names come from
//! untrusted files, so [`normalize_relative`] refuses anything that is not a
//! plain relative path inside the package before a resolver ever sees it.

/// Supplies the bytes of a file a primary file refers to.
pub trait Sidecars {
    /// `path` is already normalised: relative, `/`-separated, no `..`, no
    /// scheme. Return `None` when the file is not part of the package.
    fn get(&self, path: &str) -> Option<Vec<u8>>;
}

/// A resolver with nothing in it: reading a multi-file container through this
/// succeeds only when everything it needs is embedded.
pub struct NoSidecars;

impl Sidecars for NoSidecars {
    fn get(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }
}

/// A resolver over an in-memory set of `(path, bytes)` pairs, as a written
/// [`Package`] provides them.
pub struct MemorySidecars<'a> {
    pub files: &'a [PackageFile],
}

impl Sidecars for MemorySidecars<'_> {
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files.iter().find(|f| f.path == path).map(|f| f.bytes.clone())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackageFile {
    /// Relative path; the primary file's references resolve against it.
    pub path: String,
    pub bytes: Vec<u8>,
}

/// A written model: the primary file first, then every sidecar it references.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Package {
    pub files: Vec<PackageFile>,
}

impl Package {
    pub fn single(path: impl Into<String>, bytes: Vec<u8>) -> Package {
        Package { files: vec![PackageFile { path: path.into(), bytes }] }
    }

    pub fn primary(&self) -> &PackageFile {
        &self.files[0]
    }

    pub fn sidecars(&self) -> &[PackageFile] {
        &self.files[1..]
    }

    pub fn resolver(&self) -> MemorySidecars<'_> {
        MemorySidecars { files: &self.files }
    }
}

/// Percent-decode a URI path component (RFC 3986). Invalid escapes are kept
/// verbatim rather than guessed at.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| -> Option<u8> {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Turn a reference found inside a file into a safe package-relative path, or
/// `None` when it points outside the package.
///
/// Refused: anything with a URI scheme (`http:`, `file:`, `C:`), absolute paths,
/// and any `..` that would climb above the package root. Backslashes are read
/// as separators because Windows exporters write them in OBJ and Collada files,
/// and `./` segments are dropped.
pub fn normalize_relative(reference: &str) -> Option<String> {
    let r = reference.trim();
    if r.is_empty() || r.len() > 4096 {
        return None;
    }
    // A scheme is letters followed by ':' before any '/'. That also catches a
    // Windows drive letter, which is just as much "not in the package".
    if let Some(colon) = r.find(':') {
        if !r[..colon].contains('/') && !r[..colon].contains('\\') {
            return None;
        }
    }
    let decoded = percent_decode(r);
    let unified = decoded.replace('\\', "/");
    if unified.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in unified.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                parts.pop()?;
            }
            s => {
                if s.chars().any(|c| c.is_control()) {
                    return None;
                }
                parts.push(s);
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Resolve `reference` relative to the directory of `base` (a package path).
pub fn resolve_relative(base: &str, reference: &str) -> Option<String> {
    match base.rfind('/') {
        Some(slash) => normalize_relative(&format!("{}/{}", &base[..slash], reference)),
        None => normalize_relative(reference),
    }
}

/// A file name that every filesystem and every URL accepts, for sidecars a
/// writer names itself. Keeps letters, digits, `-`, `_` and `.`.
pub fn safe_file_stem(name: &str, fallback: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(64)
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        fallback.to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_normalised() {
        assert_eq!(normalize_relative("tex/a.png").as_deref(), Some("tex/a.png"));
        assert_eq!(normalize_relative("./tex/../a.png").as_deref(), Some("a.png"));
        assert_eq!(normalize_relative("tex\\a.png").as_deref(), Some("tex/a.png"));
        assert_eq!(normalize_relative("my%20image.png").as_deref(), Some("my image.png"));
    }

    #[test]
    fn anything_outside_the_package_is_refused() {
        for bad in [
            "../secret",
            "a/../../b",
            "/etc/passwd",
            "\\\\server\\share\\x.png",
            "http://example.com/x.png",
            "file:///etc/passwd",
            "C:\\models\\x.png",
            "data:image/png;base64,AAAA",
            "",
            "%2e%2e/secret",
        ] {
            assert_eq!(normalize_relative(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn resolution_is_relative_to_the_referring_file() {
        assert_eq!(resolve_relative("models/a.gltf", "a.bin").as_deref(), Some("models/a.bin"));
        assert_eq!(resolve_relative("a.gltf", "tex/b.png").as_deref(), Some("tex/b.png"));
        assert_eq!(resolve_relative("m/a.mtl", "../b.png").as_deref(), Some("b.png"));
        assert_eq!(resolve_relative("a.mtl", "../b.png"), None);
    }

    #[test]
    fn safe_stems_keep_only_portable_characters() {
        assert_eq!(safe_file_stem("Base Color (1)", "tex"), "Base_Color__1");
        assert_eq!(safe_file_stem("../../etc", "tex"), "etc");
        assert_eq!(safe_file_stem("", "tex"), "tex");
    }
}
