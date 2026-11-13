//! The engine/server boundary inside litegen-core, enforced.
//!
//! litegen-core holds two kinds of code. The engine turns a request into a
//! result: the provider adapters, the catalog's capabilities and pricing, the
//! shared types, the 3D library, input materializing and 3D delivery. The
//! server wraps it for many users: the HTTP API, auth, access, the database,
//! management, hooks, MCP, config and the background job loop.
//!
//! The server may use the engine. The engine names no server module, so it
//! stays testable without a database, and lifting it into its own crate stays
//! a move rather than a rewrite (docs/superpowers/specs/
//! 2026-11-07-engine-boundary-design.md, .claude/rules/engine-boundary.md).
//! One `use crate::db::…` in a hurry would weld the two together with no
//! test failing to say so, so this is a test. Like mesh/isolation_tests.rs,
//! it reads the files at test time and reports every violation at once with
//! its line.
//!
//! It reads paths the way the compiler does: comments and the insides of
//! string and char literals are not code, a group written over several lines
//! is one group, and a `super::` path is resolved from the module it is
//! written in, inline modules included. Engine code may name only the engine:
//! the folders on [`ENGINE_DIRS`] and proxy's entries on
//! [`ENGINE_PROXY_FILES`]. What it does not chase is a crossing made on
//! purpose: an alias of the crate (`use crate as root;`), a path a macro
//! builds, `include!`. Those still cross.

use std::path::{Component, Path, PathBuf};

/// Engine folders under src/: every `.rs` file in them, at any depth, is
/// engine code. (mesh/ also keeps its own, stricter test.)
const ENGINE_DIRS: &[&str] = &["providers", "capabilities", "types", "util", "mesh", "store"];

/// The engine's entries in proxy/: files, and folders written `name/`, every
/// `.rs` file in which is engine code. Every other entry there is on
/// [`SERVER_PROXY_FILES`]: a new one has to go on one list or the other,
/// which is the moment to decide which side it is on.
const ENGINE_PROXY_FILES: &[&str] = &[
    "materializer.rs",
    "materializer_tests.rs",
    "preserve.rs",
    "image_edit.rs",
    "model3d_convert.rs",
    "model3d_deliver.rs",
    "model3d_source.rs",
    "circuit_breaker.rs",
    "circuit_breaker_tests.rs",
    "ref_publish.rs",
    "segmentation.rs",
    "video_probe.rs",
];

/// The server's entries in proxy/: the request path, the job loop, the
/// storage backends and the wiring from config.
const SERVER_PROXY_FILES: &[&str] = &[
    "mod.rs",
    "registry.rs",
    "router.rs",
    "router_tests.rs",
    "cache.rs",
    "storage.rs",
    "model3d_stages.rs",
    "delivery_lease.rs",
    "config_conformance.rs",
    "webhook.rs",
    "poller.rs",
    "retention.rs",
    "aws_sts.rs",
];

/// This file: the scanner, which necessarily spells the paths it looks for.
const SELF: &str = "engine_boundary_tests.rs";

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The module name of a file or a proxy/ entry: `router.rs` is `router`,
/// `jobs/` is `jobs`.
fn stem(entry: &str) -> &str {
    entry.trim_end_matches('/').trim_end_matches(".rs")
}

/// Every engine file, relative to src/, sorted.
fn engine_files() -> Vec<PathBuf> {
    engine_files_in(&src_dir(), ENGINE_DIRS, ENGINE_PROXY_FILES)
}

/// Every engine file under `src`, relative to it, sorted.
fn engine_files_in(src: &Path, dirs: &[&str], proxy: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in dirs {
        collect_rs(&src.join(dir), &mut files);
    }
    for entry in proxy {
        match entry.strip_suffix('/') {
            Some(folder) => collect_rs(&src.join("proxy").join(folder), &mut files),
            None => files.push(src.join("proxy").join(entry)),
        }
    }
    let mut rel: Vec<PathBuf> = files.into_iter().map(|p| p.strip_prefix(src).unwrap().to_path_buf()).collect();
    rel.sort();
    rel
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} is an engine folder but unreadable: {e}", dir.display()));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// What a proxy/ folder holds that needs a side: its `.rs` files, and its
/// folders as `name/`, sorted.
fn proxy_entries(dir: &Path) -> Vec<String> {
    let mut entries: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                Some(format!("{name}/"))
            } else {
                name.ends_with(".rs").then_some(name)
            }
        })
        .collect();
    entries.sort();
    entries
}

/// The module a file is by its place on disk: `providers/image/fal.rs` is
/// `providers::image::fal`, and a `mod.rs` is its folder's module.
fn module_of(rel: &Path) -> Vec<String> {
    let mut parts: Vec<String> = rel.iter().map(|s| s.to_string_lossy().into_owned()).collect();
    let file = parts.pop().unwrap_or_default();
    if stem(&file) != "mod" {
        parts.push(stem(&file).to_string());
    }
    parts
}

/// A file's code: comments dropped, and the insides of string and char
/// literals blanked unless `keep_strings`. Newlines stay, so an offset still
/// maps to its line. A `//` in a string (a URL) is no comment, and a path in
/// a string or in a `/* */` comment is no code.
fn code_of(src: &str, keep_strings: bool) -> String {
    let s: Vec<char> = src.chars().collect();
    let inside = |c: char| if keep_strings || c == '\n' { c } else { ' ' };
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == '/' && s.get(i + 1) == Some(&'/') {
            while i < s.len() && s[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && s.get(i + 1) == Some(&'*') {
            // Block comments nest.
            let mut depth = 0usize;
            while i < s.len() {
                if s[i] == '/' && s.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if s[i] == '*' && s.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if s[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
            }
            out.push(' ');
        } else if let Some(hashes) = raw_string_at(&s, i) {
            // r"…", r#"…"#, and br/cr ones (their b or c is already out):
            // no escapes, and the end is a quote with as many #s.
            let open = i + hashes + 2;
            out.extend(&s[i..open]);
            i = open;
            while i < s.len() {
                if s[i] == '"' && (1..=hashes).all(|k| s.get(i + k) == Some(&'#')) {
                    out.extend(&s[i..i + hashes + 1]);
                    i += hashes + 1;
                    break;
                }
                out.push(inside(s[i]));
                i += 1;
            }
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < s.len() {
                match s[i] {
                    '\\' => {
                        out.extend(s[i..(i + 2).min(s.len())].iter().map(|&e| inside(e)));
                        i += 2;
                    }
                    '"' => {
                        out.push('"');
                        i += 1;
                        break;
                    }
                    other => {
                        out.push(inside(other));
                        i += 1;
                    }
                }
            }
        } else if c == '\'' && (s.get(i + 1) == Some(&'\\') || s.get(i + 2) == Some(&'\'')) {
            // A char literal: 'x', '"', '\'', '\u{…}'. A lifetime ('a) is code.
            out.push('\'');
            i += 1;
            if s.get(i) == Some(&'\\') {
                out.extend(s[i..(i + 2).min(s.len())].iter().map(|&e| inside(e)));
                i += 2;
            }
            while i < s.len() && s[i] != '\'' {
                out.push(inside(s[i]));
                i += 1;
            }
            out.push('\'');
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The number of #s of a raw string starting at `s[i]`, if one does: an `r`
/// that starts a token (or follows a `b` or `c` that does), then #s, then a
/// quote. `r#type` is an identifier.
fn raw_string_at(s: &[char], i: usize) -> Option<usize> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    if s[i] != 'r' {
        return None;
    }
    let starts_token = match i {
        0 => true,
        _ if !ident(s[i - 1]) => true,
        1 => matches!(s[0], 'b' | 'c'),
        _ => matches!(s[i - 1], 'b' | 'c') && !ident(s[i - 2]),
    };
    if !starts_token {
        return None;
    }
    let hashes = s[i + 1..].iter().take_while(|&&c| c == '#').count();
    (s.get(i + 1 + hashes) == Some(&'"')).then_some(hashes)
}

/// The inline modules in a file's code, as `(open, close, name)`: the offsets
/// of the braces of each `mod name { … }`. (A `mod name;` is another file.)
fn inline_mods(code: &str) -> Vec<(usize, usize, String)> {
    let mut mods = Vec::new();
    for i in starts(code, "mod ") {
        let rest = code[i + "mod ".len()..].trim_start();
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
        let Some(body) = rest[name.len()..].trim_start().strip_prefix('{') else { continue };
        if name.is_empty() {
            continue;
        }
        let open = code.len() - body.len() - 1;
        let mut depth = 0usize;
        let close = code[open..]
            .char_indices()
            .find_map(|(j, c)| {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(open + j);
                        }
                    }
                    _ => {}
                }
                None
            })
            .unwrap_or(code.len());
        mods.push((open, close, name));
    }
    mods
}

/// The module the code at offset `at` is in: the file's module, then each
/// inline module around it, outermost first.
fn module_at(module: &[String], mods: &[(usize, usize, String)], at: usize) -> Vec<String> {
    let inner = mods.iter().filter(|(open, close, _)| *open < at && at < *close).map(|(_, _, name)| name.clone());
    module.iter().cloned().chain(inner).collect()
}

/// A file an engine file includes with `#[path = "…"] mod name;`.
struct Included {
    /// The engine file with the attribute.
    by: PathBuf,
    /// The file it includes, relative to src/.
    file: PathBuf,
    /// The module that makes it: the includer's module plus `name`.
    module: Vec<String>,
}

/// `a/../b/./c.rs` as `b/c.rs`.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Files an engine file includes with `#[path = "…"] mod name;`, each with
/// the module that makes it: the includer's module plus `name`, wherever the
/// file sits. The path is relative to the includer's folder.
fn path_included(files: &[PathBuf]) -> Vec<Included> {
    const ATTR: &str = "#[path = \"";
    let mut out = Vec::new();
    for rel in files {
        let src = std::fs::read_to_string(src_dir().join(rel)).unwrap_or_else(|e| panic!("{} is engine code but unreadable: {e}", rel.display()));
        let code = code_of(&src, true);
        for (i, _) in code.match_indices(ATTR) {
            let rest = &code[i + ATTR.len()..];
            let Some(end) = rest.find('"') else { continue };
            let after = &rest[end..];
            let Some(at) = after.match_indices("mod ").map(|(j, _)| j).find(|&j| after[..j].ends_with(char::is_whitespace)) else { continue };
            let name: String = after[at + "mod ".len()..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let file = normalize(&rel.parent().unwrap_or(Path::new("")).join(&rest[..end]));
            out.push(Included { by: rel.clone(), file, module: module_of(rel).into_iter().chain([name]).collect() });
        }
    }
    out
}

/// Every included file that is not engine code, as a violation: it becomes
/// part of an engine module, but nothing checks it as engine code.
fn included_outside(included: &[Included], files: &[PathBuf]) -> Vec<String> {
    included
        .iter()
        .filter(|inc| !files.contains(&inc.file))
        .map(|inc| format!("{}: includes {} with #[path], and {} is not engine code", inc.by.display(), inc.file.display(), inc.file.display()))
        .collect()
}

/// The module `rel` is: the one its includer made it, or its place on disk.
fn module_in(rel: &Path, included: &[Included]) -> Vec<String> {
    included.iter().find(|inc| inc.file == rel).map(|inc| inc.module.clone()).unwrap_or_else(|| module_of(rel))
}

/// Splits `s` at the commas outside any braces.
fn split_top(s: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0usize, 0usize, Vec::new());
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Every path a text names, read from just after a `crate::` or `super::`:
/// `a::b::C` is `[a, b, C]`, `a::{b::C, d}` is `[a, b, C]` and `[a, d]`, and
/// a glob `a::*` is `[a, *]`. The blank after a group's trailing comma is no
/// member.
fn expand(text: &str) -> Vec<Vec<String>> {
    let mut prefix: Vec<String> = Vec::new();
    let mut rest = text.trim_start();
    loop {
        if let Some(group) = rest.strip_prefix('{') {
            let mut depth = 0usize;
            let mut end = group.len();
            for (i, c) in group.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' if depth == 0 => {
                        end = i;
                        break;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            return split_top(&group[..end])
                .into_iter()
                .filter(|member| !member.trim().is_empty())
                .flat_map(|member| expand(member).into_iter().map(|tail| prefix.iter().cloned().chain(tail).collect::<Vec<_>>()).collect::<Vec<_>>())
                .collect();
        }
        let ident: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if ident.is_empty() {
            if rest.starts_with('*') {
                prefix.push("*".to_string());
            }
            break;
        }
        rest = &rest[ident.len()..];
        prefix.push(ident);
        match rest.strip_prefix("::") {
            Some(r) => rest = r,
            None => break,
        }
    }
    vec![prefix]
}

/// Where `needle` starts a path of its own in `code`: not inside a longer
/// identifier (`mycrate::`), and not after another `::`.
fn starts(code: &str, needle: &str) -> Vec<usize> {
    code.match_indices(needle)
        .map(|(i, _)| i)
        .filter(|&i| {
            let before = code[..i].chars().next_back();
            let after_dollar = before == Some('$');
            !(before.is_some_and(|c| c.is_alphanumeric() || c == '_') || code[..i].ends_with("::")) || after_dollar
        })
        .collect()
}

/// A path from the crate root that is not the engine's: a first segment
/// outside [`ENGINE_DIRS`] and `proxy` (a server module, an item of lib.rs, a
/// glob of the root), or a `proxy::` that names anything but an engine entry
/// (a server file, a server item re-exported at the proxy root, a glob of
/// the proxy root).
fn crate_path_crosses(path: &[String]) -> bool {
    match path {
        [] => false,
        [first, rest @ ..] if first == "proxy" => match rest.first() {
            Some(second) => !ENGINE_PROXY_FILES.iter().any(|f| stem(f) == second),
            None => true,
        },
        [first, ..] => !ENGINE_DIRS.contains(&first.as_str()),
    }
}

/// Every server path an engine file names, as `file:line: path`, in line
/// order. `module` is the module the file is ([`module_in`]). The file is
/// read whole, so a group written over several lines is one group; a path is
/// reported at the line it starts on.
fn violations_in(rel: &Path, module: &[String], src: &str) -> Vec<String> {
    let code = code_of(src, false);
    let mods = inline_mods(&code);
    let line = |i: usize| code[..i].matches('\n').count() + 1;
    let mut found = Vec::new();
    for i in starts(&code, "crate::") {
        // `pub(in crate::proxy)` limits who sees an item; it names nothing.
        if code[..i].trim_end().strip_suffix("in").is_some_and(|before| before.trim_end().ends_with("pub(")) {
            continue;
        }
        for path in expand(&code[i + "crate::".len()..]) {
            if crate_path_crosses(&path) {
                found.push((line(i), format!("crate::{}", path.join("::"))));
            }
        }
    }
    // `self::super::` is `super::`.
    let selves = starts(&code, "self::").into_iter().map(|i| i + "self::".len()).filter(|&i| code[i..].starts_with("super::"));
    for i in starts(&code, "super::").into_iter().chain(selves) {
        let mut rest = &code[i..];
        let mut ups = 0;
        while let Some(r) = rest.strip_prefix("super::") {
            ups += 1;
            rest = r;
        }
        let here = module_at(module, &mods, i);
        if ups > here.len() {
            continue;
        }
        for tail in expand(rest).into_iter().filter(|tail| !tail.is_empty()) {
            let resolved: Vec<String> = here[..here.len() - ups].iter().cloned().chain(tail.iter().cloned()).collect();
            if crate_path_crosses(&resolved) {
                found.push((line(i), format!("{}{}", "super::".repeat(ups), tail.join("::"))));
            }
        }
    }
    found.sort_by_key(|(n, _)| *n);
    found.into_iter().map(|(n, path)| format!("{}:{n}: {path}", rel.display())).collect()
}

#[test]
fn engine_code_names_no_server_module() {
    let mut violations = Vec::new();
    let files = engine_files();
    let included = path_included(&files);
    violations.extend(included_outside(&included, &files));
    for rel in &files {
        if rel.file_name().is_some_and(|f| f == SELF) {
            continue;
        }
        let path = src_dir().join(rel);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} is engine code but unreadable: {e}", path.display()));
        violations.extend(violations_in(rel, &module_in(rel, &included), &src));
    }
    assert!(
        violations.is_empty(),
        "engine code must name only the engine: ENGINE_DIRS and proxy's ENGINE_PROXY_FILES. Everything \
         else is the server's (api, auth, access, config, db, hooks, management, mcp, observability, \
         lib.rs's own items, proxy's server files and its root). Move the helper it needs into the \
         engine, or take the value as a parameter. See .claude/rules/engine-boundary.md.\n\n{} reference(s):\n{}",
        violations.len(),
        violations.join("\n"),
    );
}

#[test]
fn every_proxy_file_is_classified() {
    let on_disk = proxy_entries(&src_dir().join("proxy"));
    let mut listed: Vec<String> = ENGINE_PROXY_FILES.iter().chain(SERVER_PROXY_FILES).map(|s| s.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed, "every file and folder in src/proxy must be on ENGINE_PROXY_FILES or SERVER_PROXY_FILES, once (a folder as `name/`)");
}

#[test]
fn the_lists_name_real_folders_and_files() {
    for dir in ENGINE_DIRS {
        assert!(src_dir().join(dir).is_dir(), "ENGINE_DIRS names src/{dir}, which is not a folder");
    }
    for entry in ENGINE_PROXY_FILES.iter().chain(SERVER_PROXY_FILES) {
        let path = src_dir().join("proxy").join(entry.trim_end_matches('/'));
        let real = if entry.ends_with('/') { path.is_dir() } else { path.is_file() };
        assert!(real, "the proxy lists name {entry}, which does not exist");
    }
}

#[test]
fn the_scanner_reads_paths_the_way_rust_writes_them() {
    let rel = Path::new("proxy/materializer.rs");
    let v = |src: &str| violations_in(rel, &module_of(rel), src);
    // Plain, grouped and nested paths, and the engine's own modules.
    assert_eq!(v("use crate::db::DatabaseStore;"), ["proxy/materializer.rs:1: crate::db::DatabaseStore"]);
    assert_eq!(v("use crate::{config::Mode, types::GenerationStatus};"), ["proxy/materializer.rs:1: crate::config::Mode"]);
    assert_eq!(v("use crate::proxy::{storage::ImageStorage, preserve::Preserve};"), ["proxy/materializer.rs:1: crate::proxy::storage::ImageStorage"]);
    assert!(v("use crate::providers::image::ImageProvider; let x = crate::mesh::glb::read(b);").is_empty());
    // A server item re-exported at the proxy root is still the server's.
    assert_eq!(v("let r: crate::proxy::ProxyRouter;"), ["proxy/materializer.rs:1: crate::proxy::ProxyRouter"]);
    // `super::` from proxy/ into a server file, and up to a server module.
    assert_eq!(v("use super::router::sha256_hex;"), ["proxy/materializer.rs:1: super::router::sha256_hex"]);
    assert_eq!(v("use super::super::api::handlers;"), ["proxy/materializer.rs:1: super::super::api::handlers"]);
    // A test module's own items, comments, and longer identifiers are not paths out.
    assert!(v("mod tests {\n    use super::*;\n    use super::materialize;\n}").is_empty());
    assert!(v("// crate::db::DatabaseStore is not code").is_empty());
    assert!(v("use mycrate::db::X; a::crate::db;").is_empty());
}

#[test]
fn groups_written_over_several_lines_are_read_whole() {
    let rel = Path::new("proxy/materializer.rs");
    let v = |src: &str| violations_in(rel, &module_of(rel), src);
    // rustfmt breaks a long group over lines; each member is still a path.
    assert_eq!(v("use crate::{\n    types::GenerationStatus,\n    db::DatabaseStore,\n};"), ["proxy/materializer.rs:1: crate::db::DatabaseStore"]);
    assert_eq!(v("use super::{\n    preserve::Preserve,\n    router::sha256_hex,\n};"), ["proxy/materializer.rs:1: super::router::sha256_hex"]);
    // A trailing comma is not a member: this group names engine files only.
    assert!(v("use crate::proxy::{\n    preserve::Preserve,\n    model3d_convert::convert,\n};").is_empty());
    // A comment in a group hides itself, not the next line.
    assert_eq!(v("use crate::{\n    types::X, // was crate::config::Old\n    db::DatabaseStore,\n};"), ["proxy/materializer.rs:1: crate::db::DatabaseStore"]);
    // A path is reported at the line it starts on.
    assert_eq!(v("use crate::types::X;\n\nfn f() { crate::config::load(); }"), ["proxy/materializer.rs:3: crate::config::load"]);
}

#[test]
fn a_file_included_with_path_is_its_includers_child() {
    // circuit_breaker.rs includes circuit_breaker_tests.rs as its own
    // `mod circuit_breaker_tests`, so `super::super` there is proxy, and
    // proxy's router is the server's.
    let rel = Path::new("proxy/circuit_breaker_tests.rs");
    let module = module_in(rel, &path_included(&engine_files()));
    assert_eq!(module, ["proxy", "circuit_breaker", "circuit_breaker_tests"]);
    assert_eq!(violations_in(rel, &module, "use super::super::router::sha256_hex;"), ["proxy/circuit_breaker_tests.rs:1: super::super::router::sha256_hex"]);
}

#[test]
fn comments_and_strings_are_not_code() {
    let rel = Path::new("proxy/materializer.rs");
    let v = |src: &str| violations_in(rel, &module_of(rel), src);
    // A `//` in a string is a URL, not a comment: the code after it counts.
    assert_eq!(v(r#"let u = "https://api.example.com"; let m = crate::config::Mode::Single;"#), ["proxy/materializer.rs:1: crate::config::Mode::Single"]);
    // A path in a string, a raw string or a block comment is not code.
    assert!(v(r#"let s = "crate::db::DatabaseStore";"#).is_empty());
    assert!(v(r##"let s = r#"say "crate::db::X""#;"##).is_empty());
    assert!(v(r#"let s = "a \" crate::db::X";"#).is_empty());
    assert!(v("/* crate::db::X /* nested */ crate::db::Y */ let x = 1;").is_empty());
    // A block comment keeps its lines.
    assert_eq!(v("/*\n crate::db::X\n*/\nuse crate::config::Mode;"), ["proxy/materializer.rs:4: crate::config::Mode"]);
    // A quote in a char literal opens no string, and a lifetime is no char.
    assert_eq!(v(r#"let q = '"'; fn f<'a>(x: &'a str) -> &'a str { crate::db::name(x) }"#), ["proxy/materializer.rs:1: crate::db::name"]);
}

#[test]
fn super_paths_resolve_from_the_module_they_are_in() {
    let at = |file: &str, src: &str| violations_in(Path::new(file), &module_of(Path::new(file)), src);
    // In an inline module, `super` is the file's own module.
    assert_eq!(at("proxy/materializer.rs", "mod tests {\n    use super::super::router::sha256_hex;\n}"), ["proxy/materializer.rs:2: super::super::router::sha256_hex"]);
    assert!(at("providers/mod.rs", "#[cfg(test)]\nmod tests {\n    use super::auth::AuthSpec;\n    use super::*;\n}").is_empty());
    // A glob of the proxy root or of the crate root takes in the server.
    assert_eq!(at("proxy/materializer_tests.rs", "use super::*;"), ["proxy/materializer_tests.rs:1: super::*"]);
    assert_eq!(at("store/mod.rs", "use super::*;"), ["store/mod.rs:1: super::*"]);
    // `self::super` is `super`.
    assert_eq!(at("proxy/materializer.rs", "use self::super::router::X;"), ["proxy/materializer.rs:1: super::router::X"]);
}

#[test]
fn globs_take_in_what_they_name_and_visibility_names_nothing() {
    let rel = Path::new("proxy/materializer.rs");
    let v = |src: &str| violations_in(rel, &module_of(rel), src);
    assert_eq!(v("use crate::*;"), ["proxy/materializer.rs:1: crate::*"]);
    assert_eq!(v("use crate::proxy::*;"), ["proxy/materializer.rs:1: crate::proxy::*"]);
    assert!(v("use crate::types::*; use crate::proxy::preserve::*;").is_empty());
    // `pub(in crate::proxy)` limits who sees an item; it names nothing.
    assert!(v("pub(in crate::proxy) fn f() {}").is_empty());
}

#[test]
fn a_folder_in_proxy_needs_a_side_and_its_files_are_read() {
    let src = tempfile::tempdir().unwrap();
    let proxy = src.path().join("proxy");
    std::fs::create_dir_all(proxy.join("jobs")).unwrap();
    std::fs::create_dir_all(src.path().join("providers")).unwrap();
    for file in ["proxy/m.rs", "proxy/notes.txt", "proxy/jobs/run.rs", "providers/x.rs"] {
        std::fs::write(src.path().join(file), "").unwrap();
    }
    assert_eq!(proxy_entries(&proxy), ["jobs/", "m.rs"]);
    let files = engine_files_in(src.path(), &["providers"], &["jobs/", "m.rs"]);
    assert_eq!(files, [PathBuf::from("providers/x.rs"), PathBuf::from("proxy/jobs/run.rs"), PathBuf::from("proxy/m.rs")]);
}

#[test]
fn including_a_file_outside_the_engine_is_a_violation() {
    let module = |m: &[&str]| m.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let included = [
        Included { by: "proxy/circuit_breaker.rs".into(), file: "proxy/circuit_breaker_tests.rs".into(), module: module(&["proxy", "circuit_breaker", "circuit_breaker_tests"]) },
        Included { by: "proxy/materializer.rs".into(), file: "db/raw.rs".into(), module: module(&["proxy", "materializer", "raw"]) },
    ];
    let files = [PathBuf::from("proxy/circuit_breaker_tests.rs"), PathBuf::from("proxy/materializer.rs")];
    assert_eq!(included_outside(&included, &files), ["proxy/materializer.rs: includes db/raw.rs with #[path], and db/raw.rs is not engine code"]);
}
