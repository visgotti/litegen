# PLY test vectors

Read by the tests in `../../ply.rs` (`include_bytes!`), so that the reader meets
a big-endian PLY file that litegen did not write. The other big-endian files
those tests read are built in `ply.rs` itself: written byte by byte, or
converted by its test helper `to_big_endian`.

Licence: the content is litegen's own `cube` fixture (`../../fixtures.rs`),
re-encoded by our own script, so the file may be committed. No third-party
content is involved.

## cube_be_struct.ply

- **What:** the `cube` fixture as `ply::write_scene` wrote it on 2026-11-04
  (`binary_little_endian 1.0`, 859 bytes, SHA-1
  `bfdf4b2138349c6db5effe168aa2d0ebd540f120`), rewritten as
  `binary_big_endian 1.0`: 856 bytes, SHA-1
  `b399925b748e03a0123ce3c16917190dd2dad703`. Only the header's format line
  changes. Every value of the body, list counts included, is read with a
  little-endian `struct` format (`'<'`) and written with a big-endian one
  (`'>'`).
- **Script:** `make_be.py`, beside this file. It uses the standard library
  only (`struct` and `sys`) and parses the header itself: element counts,
  scalar and list properties, and PLY's eight types under both spellings
  (`char`/`int8` … `double`/`float64`). It stops without writing anything on
  a header line it does not understand, or on a body that ends early or has
  bytes left over.
- **Python:** 3.14.7 (`/opt/homebrew/bin/python3`, Homebrew), run with `-I`.
- **Read by:** `an_independently_written_big_endian_file_reads` in `ply.rs`.
  The file must equal, byte for byte, what `to_big_endian` makes of the
  fixture's PLY, which is checked first, and must read as that PLY reads,
  scene and warnings alike.

## How it is made

The source is what the tests call `fixture_ply("cube", &fixtures::cube())`:
the fixture reduced by `MeshFormat::Ply.prepare`, as `mesh::write_scene`
reduces it, then written by `ply::write_scene`. `mesh/` does no I/O outside
its tests, so an ignored test in `ply.rs`,
`writes_the_cube_ply_to_the_file_named_by_the_environment`, writes those bytes
to the path in `PLY_CUBE_LE_OUT`. One cargo command dumps them, and the script
converts them. From `litegen-core`:

```sh
PLY_CUBE_LE_OUT=/tmp/cube_le.ply CARGO_INCREMENTAL=0 cargo test --lib writes_the_cube_ply_to_the_file_named_by_the_environment -- --ignored
/opt/homebrew/bin/python3 -I src/mesh/testdata/ply/make_be.py /tmp/cube_le.ply src/mesh/testdata/ply/cube_be_struct.ply
```

The committed file was made this way on 2026-11-04, except that the dump came
from a temporary ignored test with the same body (`PLY_CUBE_LE_OUT`, then
`std::fs::write` of the cube's PLY), which the permanent test replaced. The
sizes, hashes and Python version above are that run's.

## Making it again

The file pins the writer's output for the cube. If that output ever changes
(the `cube` fixture, `prepare`, the sRGB encoding or the PLY writer), the test
fails first with "cube_be_struct.ply is stale". Run the two commands above,
check that `an_independently_written_big_endian_file_reads` passes, and update
the sizes, hashes and Python version here.
