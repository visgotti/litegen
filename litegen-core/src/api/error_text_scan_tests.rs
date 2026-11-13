//! SEC-DBERR's guard: no 500 an API or MCP handler sends carries an error's
//! own text, and no failure record a tenant reads (the request log) keeps
//! it. A database error names tables, columns and constraints, can echo
//! the caller's input, and a panic's text names code; the server log has
//! them, the caller gets a fixed message. A source scan in the style of
//! `engine_boundary_tests.rs`: it catches the shapes people write by hand,
//! and names the file and line.

use std::path::{Path, PathBuf};

/// Shapes that put an error's text in a 500 body.
fn sends_error_text(line: &str) -> bool {
    line.contains("error_response(&e.to_string(), 500)")
        || line.contains("error_page(&e.to_string())")
        || (line.contains("INTERNAL_SERVER_ERROR") && line.contains("&e.to_string()"))
}

/// Shapes that keep an error's own text in a record a tenant reads (a failed
/// request's log row and error artifact, `/v1/logs`): handlers review #7. A
/// router error's text can be a panic's; `ProxyError::logged_message` is
/// what such a record keeps. `error: &e.to_string()` is how the four failed
/// generation rows were written before.
fn keeps_error_text(line: &str) -> bool {
    line.contains("error: &e.to_string()") || (line.contains("log_request(") && line.contains("e.to_string()"))
}

/// Sites another area of the 2026-11-11 review fixes. Each must still be
/// there: once it's fixed, take it off this list.
const PENDING: &[(&str, &str)] = &[];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_handler_sends_an_errors_text_with_a_500() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src.join("api"), &mut files);
    rust_files(&src.join("mcp"), &mut files);
    files.sort();
    let mut found = Vec::new();
    let mut pending_seen = vec![false; PENDING.len()];
    for file in files {
        let rel = file.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "api/error_text_scan_tests.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (i, line) in text.lines().enumerate() {
            if !sends_error_text(line) && !keeps_error_text(line) {
                continue;
            }
            match PENDING.iter().position(|(f, l)| *f == rel && line.trim() == *l) {
                Some(p) => pending_seen[p] = true,
                None => found.push(format!("src/{rel}:{}: {}", i + 1, line.trim())),
            }
        }
    }
    assert!(
        found.is_empty(),
        "500s that send an error's own text, or failure records that keep it (log it, answer or keep a fixed message):\n{}",
        found.join("\n")
    );
    for (seen, (file, line)) in pending_seen.iter().zip(PENDING) {
        assert!(seen, "src/{file} no longer has `{line}`: take it off PENDING");
    }
}
