# Big-endian PLY Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ply.rs` reads `binary_big_endian 1.0` files exactly as it reads their little-endian twins.

**Architecture:** One `Scalar::decode(bytes, ByteOrder)` replaces `decode_le`. The binary source carries the header's byte order, and nothing else changes.

**Tech Stack:** Rust, std only (`mesh/ply.rs`); `python3 -I` with `struct` for the independent test file.

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-ply-big-endian-design.md`

**Status: done on 2026-11-04 by session litegen-5c, not committed.** Every step below
was carried out. The ledger, `.superpowers/sdd/2026-11-04-mesh-ply-big-endian/progress.md`,
records each ruling (R1–R10) with its reason. The build differs from the steps in these
places:

- Task 2's bit-flip sweep covers the whole cube twin, since the file is only 856 bytes.
  Each of the 10,000 seeded cases flips 2–8 bits anywhere in it (seed
  `0x9E37_79B9_7F4A_7C15`). This is R3.
- Task 3's harness run covered every fixture as well as four imports:
  `cube_be_struct.ply`, three.js's `dolphins_be.ply` and `dolphins_le.ply`, and Stanford's
  Armadillo. 1026/1026 cases passed, with the native check at 0.00 % on every import (R9,
  R10). The drill scan read to `Empty`, and truncated Lucy was refused as `Malformed` at a
  7.8 MB peak.
- `make_be.py` is 82 lines, because it refuses every input it doesn't expect. It reads
  with `<` formats and writes with `>` formats only (R5).
- The doc change is the §1 row and one line in §3. §9 never named the refusal (R6).
- Three tests beyond the plan: `decode_reads_each_type_in_either_byte_order`,
  `binary_ushort_colour_is_divided_by_65535`, and the ignored
  `writes_the_cube_ply_to_the_file_named_by_the_environment`, which remakes
  `make_be.py`'s input.
- On the final tree, the negative control (`BinaryBe` decoding `Little`) fails 11 tests.
  The writer section of `ply.rs` is byte-identical to the pre-item file.
- One review nit is parked: the README's remake steps don't say to update its date and
  its provenance paragraph.

To commit on the other laptop:
- `litegen-core/src/mesh/ply.rs`;
- `docs/superpowers/3d-format-conversion.md` (this item's lines are the §1 PLY row and the
  §3 PLY bullet; other items edit the same file);
- the untracked folder `litegen-core/src/mesh/testdata/ply/` (`README.md`,
  `cube_be_struct.ply`, `make_be.py`). `ply.rs`'s tests won't build without
  `cube_be_struct.ply`.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- The writer keeps writing `binary_little_endian`.
- `testdata/ply/` is new. Load from it with `include_bytes!("testdata/ply/…")` only.

## Review Focus

1. A list whose count type is multi-byte (`ushort`, `uint`) in big-endian: the count must be swapped before it's bounded against the list cap.
2. A big-endian file with an unknown element whose properties are skipped: skipping must step by size, never decode.
3. `format binary_big_endian` with an omitted version (the reader allows that for little-endian): it must read the same way.
4. Header lines with CRLF endings in a big-endian file: the body offset after `end_header\r\n` must match little-endian's handling.
5. A file that declares big-endian but whose body is little-endian: it reads as garbage values, and must fail validation or read, never panic. Non-finite floats are already refused by `validate`.

---

### Task 1: Decode by byte order

**Files:** `litegen-core/src/mesh/ply.rs` (the `Scalar` impl, the `Encoding` enum, the header's `format` arm, the binary source).

**Interfaces:**
- Produces: `enum ByteOrder { Little, Big }`, private.
- Produces: `fn Scalar::decode(self, b: &[u8], order: ByteOrder) -> Option<f64>`, which replaces `decode_le`.
- Produces: `Encoding::BinaryBe`.

- [x] Write `every_scalar_type_reads_big_endian`: hand-written header and big-endian body, one property per type under both spellings. Pin these exact values: `char` −2, `uchar` 250, `short` −300, `ushort` 0x0102, `int` −70000, `uint` 0x01020304, `float` 1.5, `double` −2.25.
- [x] Write `big_endian_lists_read_with_every_count_type` (uchar, ushort and uint counts; a quad).
- [x] Delete `big_endian_is_named_not_misread`.
- [x] Run `cargo test --lib mesh::ply`. Expected: FAIL with the Unsupported refusal.
- [x] Implement. The `format` arm maps `binary_big_endian` → `Encoding::BinaryBe`.
- [x] Run `cargo test --lib mesh::ply`. Expected: pass.

### Task 2: Twins, bounds and robustness

**Files:** `litegen-core/src/mesh/ply.rs` (tests module).

**Interfaces:**
- Produces: test helper `fn to_big_endian(le: &[u8]) -> Vec<u8>`. It parses the header in test code, then reverses each scalar per its declared type, list counts included.

- [x] Write `big_endian_twins_read_identically`. It covers every existing binary-LE test body (the `BIN_HEAD` cases, face colours, UV aliases, strips, an unknown element) and `ply::write_scene(fixture)` for every `fixtures::all()`. Assert `read_scene_warned(be) == read_scene_warned(le)`, scenes and warnings alike.
- [x] Write `big_endian_bounds_hold`: the 4,000,000,000-vertex count, an over-cap list and an early end, each as big-endian. Assert each error kind equals the little-endian one. Then the twin of `binary_trailing_padding_is_tolerated`: big-endian with 16 trailing bytes reads `Ok`, with the same scene as little-endian.
- [x] Write `big_endian_truncation_and_bit_flips_never_panic`. Use the cube twin: every prefix, every bit of the first 4 KiB, and 10,000 seeded flips (record the seed).
- [x] Add Review Focus cases 1–5 as `big_endian_multibyte_list_counts_are_bounded`, `big_endian_unknown_elements_are_skipped_by_size`, `big_endian_without_a_version_reads`, `big_endian_with_crlf_header_reads`, `a_mislabelled_little_endian_body_fails_cleanly`.
- [x] Run `cargo test --lib mesh::ply`. Expected: pass.
- [x] Negative control: make `BinaryBe` decode `Little` in a scratch copy. The twins test and the scalar test fail. Restore by rewriting, `touch`, and rerun green.

### Task 3: An independently written file, the harness and docs

**Files:**
- `litegen-core/src/mesh/testdata/ply/make_be.py` (new)
- `litegen-core/src/mesh/testdata/ply/cube_be_struct.ply` (new, generated by the script)
- `litegen-core/src/mesh/testdata/ply/README.md` (new)
- `litegen-core/src/mesh/ply.rs` (one test)
- `docs/superpowers/3d-format-conversion.md` (the §1 table and §3; §9 never named the refusal)
- the `ply.rs` module doc

- [x] Write `make_be.py`. With `struct` `>` formats only, it writes the `cube` fixture's PLY content (read its values from `ply::write_scene` output passed as an argument, little-endian in, big-endian out, with its own header parsing) to `cube_be_struct.ply`. Run `python3 -I make_be.py <le.ply> cube_be_struct.ply`. Note the Python version in the README.
- [x] Write `an_independently_written_big_endian_file_reads`: the file's scene `==` the fixture's PLY reading.
- [x] Run the harness: `cargo run --release --example mesh_matrix -- <dir> --fixtures none --import <dir-with-cube_be_struct.ply and any research-named BE files>`, then `node verify.mjs`. Expected: the native check is 0.00 % and every direction passes.
- [x] Update the docs, and tell litegen-a3 the matrix cell changed.
- [x] Run the overview's full gate. Write the ledger (RED→GREEN names, the negative control, the harness report summary).
