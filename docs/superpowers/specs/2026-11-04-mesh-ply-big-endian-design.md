# Big-endian PLY input — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: S.

**Done on 2026-11-04** by session litegen-5c, not committed. The
[plan](../plans/2026-11-04-mesh-ply-big-endian.md) opens with a status block: where the
build differs from this design, and the files to commit.

## Why

`ply.rs` refuses `format binary_big_endian 1.0` by name (`MeshError::Unsupported`,
"binary_big_endian PLY: this module reads ascii and binary_little_endian only"). The
comment explains why: byte-swapping was "untested code on a path no modern exporter
takes". Big-endian PLY is still what older scanning pipelines and SGI-era tools wrote,
and the PLY spec allows it (paulbourke.net/dataformats/ply). A caller with such a file
gets a refusal for a container litegen otherwise reads in full.

## Where it stands today

- The header parser maps `ascii` → `Encoding::Ascii` and `binary_little_endian` →
  `Encoding::BinaryLe`. `binary_big_endian` returns the refusal above, and test
  `big_endian_is_named_not_misread` pins it.
- Binary values decode through `Scalar::decode_le`, one function for all eight types
  (`char/int8`, `uchar/uint8`, `short/int16`, `ushort/uint16`, `int/int32`,
  `uint/uint32`, `float/float32`, `double/float64`). Lists, element skipping, counts and
  budgets all go through the binary source's `scalar`, `skip_scalar` and `list`.
- The writer writes `binary_little_endian` (module doc, "Why we write
  binary_little_endian"), and that stays.

## Goals

1. Read `binary_big_endian 1.0` with exactly the coverage little-endian has: every
   scalar type and alias, list properties of any count and item type, vertex and face
   colours, UVs and their aliases, triangle strips, unknown elements skipped, and every
   hostile-input bound.
2. A big-endian file and its little-endian twin read to identical scenes, bit for bit.

## Non-goals

- Writing big-endian. Every current reader takes little-endian, and the writer's choice
  is documented.
- Any other PLY version or encoding.

## Decisions (mine)

- `Encoding` gains `BinaryBe`. The binary source holds the byte order and calls
  `Scalar::decode(self, b, order)`. `decode_le` becomes `decode(…, ByteOrder::Little)`,
  so there is one decoding function and nothing to drift apart. Single-byte types ignore
  the order.
- No other code changes: element layout, skipping, list bounds and budgets are
  byte-order independent and stay as they are.
- `big_endian_is_named_not_misread` is replaced by tests that read big-endian files.
  The refusal message disappears.

## What proves it is done

### Unit tests (ply.rs, RED first)

- `every_scalar_type_reads_big_endian`: a hand-written header with one vertex element
  holding one property of each of the eight types, under both spellings. The body is
  written byte by byte in the test (not through any swapper), for example `ushort`
  0x0102 as `[0x01, 0x02]` and `float` 1.5 as `[0x3F, 0xC0, 0x00, 0x00]`. Each value
  reads exactly, and the same file as little-endian, with its bytes reversed per
  scalar, gives the same values.
- `big_endian_lists_read_with_every_count_type`: face lists with `uchar`, `ushort` and
  `uint` counts and `int`/`uint` indices. A quad is ear-clipped into the same triangles
  as its little-endian twin.
- `big_endian_twins_read_identically`: for every binary-LE file the PLY tests already
  build (the `BIN_HEAD` cases, face colours, UV aliases, strips, unknown elements) and
  for `ply::write_scene` of every fixture, build a big-endian twin with the test helper
  `to_big_endian(le: &[u8]) -> Vec<u8>`. Both read to `==` scenes, with the same warnings.
  The helper parses the header itself, in test code independent of the reader's parser,
  and reverses every scalar per the declared types, list counts included.
- `big_endian_bounds_hold`: the existing hostile cases (a 4,000,000,000-vertex count, a
  list longer than the cap, a body that ends early), each run as big-endian, fail with
  the same error kinds as little-endian. Trailing padding is tolerated in both byte
  orders (`binary_trailing_padding_is_tolerated`, ply.rs): its big-endian twin reads
  `Ok`, with the same scene as little-endian.
- `big_endian_truncation_and_bit_flips_never_panic`: the `cube` fixture's big-endian
  twin, read at every prefix length and with every single-bit flip in the first 4 KiB
  and a seeded sample of 10,000 flips elsewhere. The result is `Ok` or `Err`, never a
  panic.

### An independent writer

- `testdata/ply/cube_be_struct.ply`: the `cube` fixture written big-endian by a short
  `python3 -I` script that uses only `struct`, reading with `<` formats and writing with
  `>` formats. The script is committed beside the file as `testdata/ply/make_be.py` (82
  lines as built, most of them refusals of unexpected input). It is our content, so it
  can be committed. A README line
  names the script and Python version. Test `an_independently_written_big_endian_file_reads`
  reads it, and its scene equals the fixture's PLY reading (positions, normals, colours,
  UVs exact).

### Render harness

- Add `cube_be_struct.ply`, plus any third-party big-endian files the research names
  (local only, never committed), to an `--import` run. three.js r183 (0.183.2)
  `PLYLoader` reads `binary_big_endian`: `PLYLoader.js:679` sets
  `little_endian = header.format === 'binary_little_endian'`, and every multi-byte read
  is `dataview.getX(at, little_endian)` (604–667). Verified by litegen-5c on
  2026-11-04. So the harness's **native** check compares our reading with three.js's
  own. It must be 0.00 % in geom and albedo, and every direction must pass.
- **Third-party big-endian files** ([research-consumers.md](../research/research-consumers.md) Q9; local only, never
  committed):
  - three.js r183 `examples/models/ply/binary/dolphins_be.ply` and its twin
    `dolphins_le.ply` (855 vertices; the npm package doesn't ship them). They must read
    to equal scenes;
  - Stanford's `Armadillo.ply` (big-endian, 172,974 vertices), rendered in every
    direction;
  - Stanford's `drill/data/drill_1.6mm_*_cyb.ply` range scans: vertices and a
    `range_grid` list element, but no faces. They must reach `MeshError::Empty`. That is
    a smoke test on a real file, not proof of the list walk. `range_grid` is `list uchar
    int`, so its counts are single bytes, and because trailing bytes are tolerated,
    `Empty` can't tell an exact walk from one that stopped short. The proof is the plan's
    `big_endian_multibyte_list_counts_are_bounded` and
    `big_endian_unknown_elements_are_skipped_by_size`. After item 10 the scans read as
    points;
  - `lucy.ply` (14,027,872 vertices, 533 MB): refused by the 128 MiB input cap before
    parsing. A truncated copy of its head is refused before allocation (Malformed from
    the record-length check, which runs before the vertex ceiling). Either refusal is
    right, provided nothing proportional to 14M vertices is allocated (`mesh_peak`).

  If none can be fetched, `cube_be_struct.ply` alone meets the overview's bar 6 for
  this item, because the item adds nothing beyond byte order.

### Negative control

- In a scratch copy, make `BinaryBe` decode little-endian. `every_scalar_type_reads_big_endian`
  and `big_endian_twins_read_identically` fail. Restore by rewriting.

### Docs

- `ply.rs` module doc and `docs/superpowers/3d-format-conversion.md`: the §1 table row
  becomes "ASCII and binary (both byte orders)", and §3's PLY bullet says
  `binary_big_endian` is read while the writer stays little-endian. §3 and §9 never
  named the refusal, so nothing else changes.
- `mesh/mod.rs` needs no change.
- Tell litegen-a3 [d8ac4c], which owns the matrix page, that its PLY "Reads" cell is now
  Kept.

### Gate

`cargo test --lib -- mesh::ply mesh::tests`, then the overview's full gate. Then the
harness `--import` run above, with 0 failures.

## Left to the builder

The `to_big_endian` helper's shape, and the exact flip-sample seed (record it in the test).
