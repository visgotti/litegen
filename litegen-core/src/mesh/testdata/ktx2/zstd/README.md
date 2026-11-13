# Zstandard test vectors for `ruzstd`

Zstandard streams made by the reference tool, libzstd's `zstd` command line program, and
the bytes that tool decodes each of them to. They are the corpus of mesh-gaps item 3's
decision D3, "`ruzstd`, fully tested": `ktx2.rs` inflates Zstandard-supercompressed KTX2
levels with the pure-Rust `ruzstd` crate (pinned `=0.9.1`, no dependencies of its own with
the features litegen builds it with), and these files show that it agrees with libzstd byte
for byte on the streams an encoder writes at the levels and options below, and that it
survives damaged ones.

The tests are in `ktx2.rs`, module `tests::zstd_corpus`. They embed every file in this
folder with `include_bytes!` and this README with `include_str!`; nothing is read from the
disk at run time.

- `ruzstd_matches_reference_zstd`: each `.zst` below decodes, through litegen's own
  Zstandard path (`inflate`, which wraps `inflate_reading`: the streaming API exactly as
  `decode_rgba8` uses it), to the bytes `zstd -d` printed, and to the sha256 the table lists.
  A stream that litegen deliberately does not take (several frames, a skippable frame, a
  window past the limit; see the end of this file) is decoded by the same streaming API in
  the way its documentation gives for it, and litegen's refusal of it is pinned
  (`inflate_refuses_what_zstd_takes_as_several_frames`, and the window check inside the
  first test).
- `the_zstd_corpus_survives_truncation_and_bit_flips`: every file cut short, with bits
  flipped, and with random bits flipped (xorshift64, seed `0x9E3779B97F4A7C15`): no panic,
  and every failure is `MeshError::Malformed`, never another error, and a stream that
  decodes decodes to the level's length. The ignored
  `the_zstd_corpus_survives_the_whole_sweep_bar` runs every cut, every bit and 10,000
  random cases of each file whose output is under 64 KiB, and a spaced share of those of the
  files whose output is 64 KiB or more (`uastc_normal`, `structured_long` and
  `far_repeats`). A file that names a window past litegen's limit (the two
  `*.stream.l22.long27.zst`) is swept through `ruzstd` with the limit raised to its window,
  as the matching test reads the intact file, so the damaged copies reach its blocks and not
  only the refusal (`a_window_past_the_limit_is_swept_with_the_limit_raised`).
  `the_sweep_fails_on_a_panic_another_error_or_a_wrong_length` shows the sweep fails on each
  of the three, and names the case.
- `the_readme_lists_the_embedded_corpus`: the tables here list exactly the files the
  tests embed, each once, with the sizes they have, and the frame counts and windows that
  `zstd -lv` printed agree with what the tests read from the bytes.
- `the_expected_outputs_are_what_zstd_printed`: each `.expected.zst` decodes to bytes
  whose sha256 is the table's, and `rle`, `random` and `mixed3` are also made again from
  their recorded generators, so those three are checked by code that has nothing to do with
  Zstandard. `sha256_matches_the_published_vectors` checks the test's own SHA-256 against
  the NIST values, and `the_comparison_catches_a_wrong_byte_a_wrong_hash_and_a_cut_stream`
  that the comparison fails on each of those three.
- `the_corpus_reaches_every_block_kind`: what the files hold, read from their bytes (the
  table at the end): the kinds of block, literals and sequence table, and what only the
  largest input reaches (a Huffman literals section with 18-bit sizes, a tree written as
  direct weights, a compressed block of 64 KiB or more, a match that reaches back more
  than 128 KiB). To count the offsets it reads the sequences itself, and holds every match
  to the input; `the_replay_holds_every_match_to_the_output` shows that hold fails on an
  output that is not the stream's.

## The tool

```
$ zstd --version
*** Zstandard CLI (64-bit) v1.5.7, by Yann Collet ***
```

Homebrew `zstd 1.5.7_1` (`/opt/homebrew/bin/zstd`), macOS on Apple silicon. Node v26.9.0
ran the script. Another zstd version may write other bytes for the same input, which is why
the tables list the sha256 of what `zstd -d` printed (the input) and not of the encodings.

## The inputs

The raw inputs are not committed: each encoding below decodes to one of them, and each
input is committed once, compressed, as `<input>.expected.zst`. `zstd -d` of every encoding
was compared with the input, byte for byte, by the script, which exits non-zero otherwise.

| file | origin | bytes | sha256 |
|---|---|---:|---|
| `uastc_normal.bin` | level 0 of `FlightHelmet_Materials_LensesMat_Normal.ktx2` (UASTC + Zstandard, CC0 1.0), inflated by `zstd -d` | 1,048,576 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_all_modes.bin` | level 0 of `uastc_all_modes.ktx2` (UASTC, scheme 0): 240 blocks, every mode; ours | 3,840 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `rle.bin` | 4,096 x `0xA5`; ours | 4,096 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `random.bin` | xorshift64 seed `0xC0FFEE1234567890`, 512 outputs as u64 little-endian; ours | 4,096 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `structured.bin` | 64 fixed-width job-log lines, xorshift64 seed `0x5EED5EED5EED5EED`; ours | 6,730 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured_long.bin` | 700 of the same lines, xorshift64 seed `0x5EED00005EED0001`; ours | 73,626 | `91e40b00dcf501d5a5604d22d7f95b7f3db6eb1f3249d0e4056b93892e585dcf` |
| `unit.bin` | a 16-byte unit and 64 more bytes, 40 times (3,200 bytes), xorshift64 seed `0x0123456789ABCDEF`; ours | 3,200 | `2628deebaa286018e909fd9778afbce55b5f350aea1f1a4aaabe39f91767569d` |
| `far_repeats.bin` | noise over 32 values, then copies of it past 128 KiB away, with the UASTC blocks of `uastc_all_modes` (421,440 bytes), xorshift64 seed `0xFA12C0DE5EED0001`; ours | 421,440 | `1e833dbf8c4eda25f82471ab48c888662fc1f2168235bc63bb54b2e2fd92930d` |
| `mixed3.bin` | `rle`, `random` and `structured` end to end; ours | 14,922 | `65dce650b79e1c1778743c6ecd9b1ab32dc1d7e4b9b948adfc22af6a1191baeb` |

The generators are `make_corpus.mjs` in this folder. xorshift64 is Marsaglia's, with shifts
13, 7 and 17 (the tests' sweeps use it too); `random` writes 512 outputs as little-endian
`u64`. `rle` is 4,096 bytes of `0xA5`, not 0, so a decoder that left its buffer zeroed
would fail it. `structured` is 64 fixed-width lines of a made-up job log whose fields are
drawn from the stream: the digits and the word choices are literals for Huffman coding, the
many short matches need FSE tables for their sequences, and the same template recurs at
the same distance on every line, which is what repeat offsets are for. `structured_long`
is 700 such lines from another seed (73,626 bytes: more than a 64 KiB block, and more
than a 32 KiB window). `unit` is a 16-byte unit and 64 random bytes, forty times over, so that every match is the
unit and every run of literals is 64 bytes but the first (80, in the same code): an encoder
writes the literal length and match length tables of that as RLE. `uastc_normal` is the one UASTC level (1 MiB); `uastc_all_modes` is
real UASTC blocks of every mode, which zstd cannot compress, so it is raw blocks of real data.

`far_repeats` (421,440 bytes) is what a block of 128 KiB or less cannot be, and the
inputs above are not. Its first 114,688 bytes are noise over 32 values (0 to 31, drawn
alike, 5 bits each), then the first 16,384 of them again to fill the block. That is a
compressed block of 71,722 bytes at `-3` and 71,795 at `-19`: past 64 KiB, with Huffman literals of 18-bit sizes
(a literals section of 16 KiB or more has a header of 5 bytes, the size format 3), and a
tree of 32 weights that are all one, which the encoder writes as direct 4-bit weights, as
it does for any alphabet whose size is a power of two (an alphabet of 20 or 24 values gets
FSE-compressed weights, as do the 116 trees of the files above). The next block is the 3,840
bytes of `uastc_all_modes` and the noise over again: one match whose offset is 134,912. The
third is about a hundred runs of fresh noise, each followed by a copy from the first block
(offsets from 135,000 to 378,682), and the last one is the tail. The offsets are all in
reach: the window of a file made from this input is its length.

## The encodings

For each of the five basic inputs (`uastc_normal`, `uastc_all_modes`, `rle`, `random`,
`structured`):

- `-1`, `-3`, `-9`, `-19` and `--ultra -22`, each with `--check` (an XXH64 checksum after
  the last block) and with `--no-check`;
- `-19 --long=27`;
- `-3 --check` and `-19 --no-check` with `--zstd=wlog=10`, a window of 1 KiB, the least
  there is. A block holds at most a window, so a 4 KiB input is four blocks. That is how a
  small input gets an RLE block (an encoder never writes one first in a frame) and a
  compressed block that reuses an earlier block's Huffman tree, tables and offsets; and the
  output is longer than the window, so the decoder has to discard what it has output;
- `-3` and `-19` fed through a redirect (`< input`), so the encoder is not told the content
  size and writes a frame without `Frame_Content_Size`, with the window its level names
  (2 MiB and 8 MiB; the file route names a window of the input's own size).

And:

- `--ultra -22 --long=27 < input` for `uastc_normal` and `structured`: frames that name a
  128 MiB window (`2^27`, the most a decoder is required to take);
- `far_repeats`, at `-3 --check` and `-19 --check`. `-19 --long=27` and the expected output
  (`--ultra -22 --long=27`) write the very same 79,025 bytes for this input, which the script
  checks, so that file is committed once, as the expected output;
- `rle.fast1000.zst`: `--fast=1000 --compress-literals`. The encoder probes every
  1,000th byte, so a long run of one byte goes out as literals, which the tool leaves
  uncompressed at that level unless told to compress them: a run of one byte is a literals
  section of the RLE kind, which no other setting reached;
- `unit`, at `-3 --check`, at `-19 --no-check` and at `-9 --check` with a 1 KiB window;
- `structured_long`, at `-3`, at `-19` with a 1 KiB window, at `-9` with a 4 KiB one and at
  `--ultra -22` with a 32 KiB one: many blocks of text-like data, which reuse trees, tables
  and offsets across blocks;
- three frames end to end (`mixed3.frames3.zst`: a different input, level and check flag
  in each) and a skippable frame first (`structured.skippable-first.zst`). The `zstd` tool
  has no option to write a skippable frame, so the script writes it from RFC 8878 section
  3.1.2 (magic `0x184D2A50`, a `u32` size, that many bytes of user data; the table has the
  `printf` that writes the same bytes, and the script runs it to check), and `zstd -d` and
  `zstd -t` accept the file.

`frames` and `skippable` are `zstd -lv`'s counts and `window` its window size in bytes (for
several frames, as it prints them). `bytes` is the size of the `.zst` file.

| file | command | frames | skippable | window | bytes | sha256 of `zstd -d` |
|---|---|---:|---:|---:|---:|---|
| `uastc_normal.l1.check.zst` | `zstd -q -f -1 --check uastc_normal.bin -o uastc_normal.l1.check.zst` | 1 | 0 | 524,288 | 411 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l1.nocheck.zst` | `zstd -q -f -1 --no-check uastc_normal.bin -o uastc_normal.l1.nocheck.zst` | 1 | 0 | 524,288 | 407 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l3.check.zst` | `zstd -q -f -3 --check uastc_normal.bin -o uastc_normal.l3.check.zst` | 1 | 0 | 1,048,576 | 392 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l3.nocheck.zst` | `zstd -q -f -3 --no-check uastc_normal.bin -o uastc_normal.l3.nocheck.zst` | 1 | 0 | 1,048,576 | 388 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l9.check.zst` | `zstd -q -f -9 --check uastc_normal.bin -o uastc_normal.l9.check.zst` | 1 | 0 | 1,048,576 | 368 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l9.nocheck.zst` | `zstd -q -f -9 --no-check uastc_normal.bin -o uastc_normal.l9.nocheck.zst` | 1 | 0 | 1,048,576 | 364 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l19.check.zst` | `zstd -q -f -19 --check uastc_normal.bin -o uastc_normal.l19.check.zst` | 1 | 0 | 1,048,576 | 369 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l19.nocheck.zst` | `zstd -q -f -19 --no-check uastc_normal.bin -o uastc_normal.l19.nocheck.zst` | 1 | 0 | 1,048,576 | 365 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l22.check.zst` | `zstd -q -f --ultra -22 --check uastc_normal.bin -o uastc_normal.l22.check.zst` | 1 | 0 | 1,048,576 | 369 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l22.nocheck.zst` | `zstd -q -f --ultra -22 --no-check uastc_normal.bin -o uastc_normal.l22.nocheck.zst` | 1 | 0 | 1,048,576 | 365 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.l19.long27.zst` | `zstd -q -f -19 --long=27 --check uastc_normal.bin -o uastc_normal.l19.long27.zst` | 1 | 0 | 1,048,576 | 378 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.w10.l3.check.zst` | `zstd -q -f -3 --zstd=wlog=10 --check uastc_normal.bin -o uastc_normal.w10.l3.check.zst` | 1 | 0 | 1,024 | 12,644 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.w10.l19.nocheck.zst` | `zstd -q -f -19 --zstd=wlog=10 --no-check uastc_normal.bin -o uastc_normal.w10.l19.nocheck.zst` | 1 | 0 | 1,024 | 10,546 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.stream.l3.zst` | `zstd -q -f -3 -c < uastc_normal.bin > uastc_normal.stream.l3.zst` | 1 | 0 | 2,097,152 | 389 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_normal.stream.l19.zst` | `zstd -q -f -19 -c < uastc_normal.bin > uastc_normal.stream.l19.zst` | 1 | 0 | 8,388,608 | 366 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_all_modes.l1.check.zst` | `zstd -q -f -1 --check uastc_all_modes.bin -o uastc_all_modes.l1.check.zst` | 1 | 0 | 3,840 | 3,854 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l1.nocheck.zst` | `zstd -q -f -1 --no-check uastc_all_modes.bin -o uastc_all_modes.l1.nocheck.zst` | 1 | 0 | 3,840 | 3,850 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l3.check.zst` | `zstd -q -f -3 --check uastc_all_modes.bin -o uastc_all_modes.l3.check.zst` | 1 | 0 | 3,840 | 3,854 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l3.nocheck.zst` | `zstd -q -f -3 --no-check uastc_all_modes.bin -o uastc_all_modes.l3.nocheck.zst` | 1 | 0 | 3,840 | 3,850 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l9.check.zst` | `zstd -q -f -9 --check uastc_all_modes.bin -o uastc_all_modes.l9.check.zst` | 1 | 0 | 3,840 | 3,854 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l9.nocheck.zst` | `zstd -q -f -9 --no-check uastc_all_modes.bin -o uastc_all_modes.l9.nocheck.zst` | 1 | 0 | 3,840 | 3,850 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l19.check.zst` | `zstd -q -f -19 --check uastc_all_modes.bin -o uastc_all_modes.l19.check.zst` | 1 | 0 | 3,840 | 3,854 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l19.nocheck.zst` | `zstd -q -f -19 --no-check uastc_all_modes.bin -o uastc_all_modes.l19.nocheck.zst` | 1 | 0 | 3,840 | 3,850 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l22.check.zst` | `zstd -q -f --ultra -22 --check uastc_all_modes.bin -o uastc_all_modes.l22.check.zst` | 1 | 0 | 3,840 | 3,854 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l22.nocheck.zst` | `zstd -q -f --ultra -22 --no-check uastc_all_modes.bin -o uastc_all_modes.l22.nocheck.zst` | 1 | 0 | 3,840 | 3,850 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.l19.long27.zst` | `zstd -q -f -19 --long=27 --check uastc_all_modes.bin -o uastc_all_modes.l19.long27.zst` | 1 | 0 | 3,840 | 3,854 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.w10.l3.check.zst` | `zstd -q -f -3 --zstd=wlog=10 --check uastc_all_modes.bin -o uastc_all_modes.w10.l3.check.zst` | 1 | 0 | 1,024 | 3,864 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.w10.l19.nocheck.zst` | `zstd -q -f -19 --zstd=wlog=10 --no-check uastc_all_modes.bin -o uastc_all_modes.w10.l19.nocheck.zst` | 1 | 0 | 1,024 | 3,860 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.stream.l3.zst` | `zstd -q -f -3 -c < uastc_all_modes.bin > uastc_all_modes.stream.l3.zst` | 1 | 0 | 2,097,152 | 3,853 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `uastc_all_modes.stream.l19.zst` | `zstd -q -f -19 -c < uastc_all_modes.bin > uastc_all_modes.stream.l19.zst` | 1 | 0 | 8,388,608 | 3,853 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `rle.l1.check.zst` | `zstd -q -f -1 --check rle.bin -o rle.l1.check.zst` | 1 | 0 | 4,096 | 23 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l1.nocheck.zst` | `zstd -q -f -1 --no-check rle.bin -o rle.l1.nocheck.zst` | 1 | 0 | 4,096 | 19 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l3.check.zst` | `zstd -q -f -3 --check rle.bin -o rle.l3.check.zst` | 1 | 0 | 4,096 | 23 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l3.nocheck.zst` | `zstd -q -f -3 --no-check rle.bin -o rle.l3.nocheck.zst` | 1 | 0 | 4,096 | 19 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l9.check.zst` | `zstd -q -f -9 --check rle.bin -o rle.l9.check.zst` | 1 | 0 | 4,096 | 23 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l9.nocheck.zst` | `zstd -q -f -9 --no-check rle.bin -o rle.l9.nocheck.zst` | 1 | 0 | 4,096 | 19 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l19.check.zst` | `zstd -q -f -19 --check rle.bin -o rle.l19.check.zst` | 1 | 0 | 4,096 | 22 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l19.nocheck.zst` | `zstd -q -f -19 --no-check rle.bin -o rle.l19.nocheck.zst` | 1 | 0 | 4,096 | 18 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l22.check.zst` | `zstd -q -f --ultra -22 --check rle.bin -o rle.l22.check.zst` | 1 | 0 | 4,096 | 22 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l22.nocheck.zst` | `zstd -q -f --ultra -22 --no-check rle.bin -o rle.l22.nocheck.zst` | 1 | 0 | 4,096 | 18 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.l19.long27.zst` | `zstd -q -f -19 --long=27 --check rle.bin -o rle.l19.long27.zst` | 1 | 0 | 4,096 | 22 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.w10.l3.check.zst` | `zstd -q -f -3 --zstd=wlog=10 --check rle.bin -o rle.w10.l3.check.zst` | 1 | 0 | 1,024 | 36 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.w10.l19.nocheck.zst` | `zstd -q -f -19 --zstd=wlog=10 --no-check rle.bin -o rle.w10.l19.nocheck.zst` | 1 | 0 | 1,024 | 31 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.stream.l3.zst` | `zstd -q -f -3 -c < rle.bin > rle.stream.l3.zst` | 1 | 0 | 2,097,152 | 22 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `rle.stream.l19.zst` | `zstd -q -f -19 -c < rle.bin > rle.stream.l19.zst` | 1 | 0 | 8,388,608 | 21 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `random.l1.check.zst` | `zstd -q -f -1 --check random.bin -o random.l1.check.zst` | 1 | 0 | 4,096 | 4,110 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l1.nocheck.zst` | `zstd -q -f -1 --no-check random.bin -o random.l1.nocheck.zst` | 1 | 0 | 4,096 | 4,106 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l3.check.zst` | `zstd -q -f -3 --check random.bin -o random.l3.check.zst` | 1 | 0 | 4,096 | 4,110 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l3.nocheck.zst` | `zstd -q -f -3 --no-check random.bin -o random.l3.nocheck.zst` | 1 | 0 | 4,096 | 4,106 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l9.check.zst` | `zstd -q -f -9 --check random.bin -o random.l9.check.zst` | 1 | 0 | 4,096 | 4,110 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l9.nocheck.zst` | `zstd -q -f -9 --no-check random.bin -o random.l9.nocheck.zst` | 1 | 0 | 4,096 | 4,106 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l19.check.zst` | `zstd -q -f -19 --check random.bin -o random.l19.check.zst` | 1 | 0 | 4,096 | 4,110 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l19.nocheck.zst` | `zstd -q -f -19 --no-check random.bin -o random.l19.nocheck.zst` | 1 | 0 | 4,096 | 4,106 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l22.check.zst` | `zstd -q -f --ultra -22 --check random.bin -o random.l22.check.zst` | 1 | 0 | 4,096 | 4,110 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l22.nocheck.zst` | `zstd -q -f --ultra -22 --no-check random.bin -o random.l22.nocheck.zst` | 1 | 0 | 4,096 | 4,106 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.l19.long27.zst` | `zstd -q -f -19 --long=27 --check random.bin -o random.l19.long27.zst` | 1 | 0 | 4,096 | 4,110 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.w10.l3.check.zst` | `zstd -q -f -3 --zstd=wlog=10 --check random.bin -o random.w10.l3.check.zst` | 1 | 0 | 1,024 | 4,120 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.w10.l19.nocheck.zst` | `zstd -q -f -19 --zstd=wlog=10 --no-check random.bin -o random.w10.l19.nocheck.zst` | 1 | 0 | 1,024 | 4,116 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.stream.l3.zst` | `zstd -q -f -3 -c < random.bin > random.stream.l3.zst` | 1 | 0 | 2,097,152 | 4,109 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `random.stream.l19.zst` | `zstd -q -f -19 -c < random.bin > random.stream.l19.zst` | 1 | 0 | 8,388,608 | 4,109 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `structured.l1.check.zst` | `zstd -q -f -1 --check structured.bin -o structured.l1.check.zst` | 1 | 0 | 6,730 | 1,684 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l1.nocheck.zst` | `zstd -q -f -1 --no-check structured.bin -o structured.l1.nocheck.zst` | 1 | 0 | 6,730 | 1,680 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l3.check.zst` | `zstd -q -f -3 --check structured.bin -o structured.l3.check.zst` | 1 | 0 | 6,730 | 1,695 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l3.nocheck.zst` | `zstd -q -f -3 --no-check structured.bin -o structured.l3.nocheck.zst` | 1 | 0 | 6,730 | 1,691 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l9.check.zst` | `zstd -q -f -9 --check structured.bin -o structured.l9.check.zst` | 1 | 0 | 6,730 | 1,550 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l9.nocheck.zst` | `zstd -q -f -9 --no-check structured.bin -o structured.l9.nocheck.zst` | 1 | 0 | 6,730 | 1,546 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l19.check.zst` | `zstd -q -f -19 --check structured.bin -o structured.l19.check.zst` | 1 | 0 | 6,730 | 1,449 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l19.nocheck.zst` | `zstd -q -f -19 --no-check structured.bin -o structured.l19.nocheck.zst` | 1 | 0 | 6,730 | 1,445 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l22.check.zst` | `zstd -q -f --ultra -22 --check structured.bin -o structured.l22.check.zst` | 1 | 0 | 6,730 | 1,449 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l22.nocheck.zst` | `zstd -q -f --ultra -22 --no-check structured.bin -o structured.l22.nocheck.zst` | 1 | 0 | 6,730 | 1,445 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.l19.long27.zst` | `zstd -q -f -19 --long=27 --check structured.bin -o structured.l19.long27.zst` | 1 | 0 | 6,730 | 1,449 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.w10.l3.check.zst` | `zstd -q -f -3 --zstd=wlog=10 --check structured.bin -o structured.w10.l3.check.zst` | 1 | 0 | 1,024 | 2,210 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.w10.l19.nocheck.zst` | `zstd -q -f -19 --zstd=wlog=10 --no-check structured.bin -o structured.w10.l19.nocheck.zst` | 1 | 0 | 1,024 | 1,844 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.stream.l3.zst` | `zstd -q -f -3 -c < structured.bin > structured.stream.l3.zst` | 1 | 0 | 2,097,152 | 1,660 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured.stream.l19.zst` | `zstd -q -f -19 -c < structured.bin > structured.stream.l19.zst` | 1 | 0 | 8,388,608 | 1,448 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `uastc_normal.stream.l22.long27.zst` | `zstd -q -f --ultra -22 --long=27 -c < uastc_normal.bin > uastc_normal.stream.l22.long27.zst` | 1 | 0 | 134,217,728 | 375 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `structured.stream.l22.long27.zst` | `zstd -q -f --ultra -22 --long=27 -c < structured.bin > structured.stream.l22.long27.zst` | 1 | 0 | 134,217,728 | 1,448 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `rle.fast1000.zst` | `zstd -q -f --fast=1000 --compress-literals --check rle.bin -o rle.fast1000.zst` | 1 | 0 | 4,096 | 24 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `unit.l3.check.zst` | `zstd -q -f -3 --check unit.bin -o unit.l3.check.zst` | 1 | 0 | 3,200 | 2,631 | `2628deebaa286018e909fd9778afbce55b5f350aea1f1a4aaabe39f91767569d` |
| `unit.l19.nocheck.zst` | `zstd -q -f -19 --no-check unit.bin -o unit.l19.nocheck.zst` | 1 | 0 | 3,200 | 2,627 | `2628deebaa286018e909fd9778afbce55b5f350aea1f1a4aaabe39f91767569d` |
| `unit.w10.l9.check.zst` | `zstd -q -f -9 --zstd=wlog=10 --check unit.bin -o unit.w10.l9.check.zst` | 1 | 0 | 1,024 | 2,673 | `2628deebaa286018e909fd9778afbce55b5f350aea1f1a4aaabe39f91767569d` |
| `structured_long.l3.check.zst` | `zstd -q -f -3 --check structured_long.bin -o structured_long.l3.check.zst` | 1 | 0 | 73,626 | 17,474 | `91e40b00dcf501d5a5604d22d7f95b7f3db6eb1f3249d0e4056b93892e585dcf` |
| `structured_long.w10.l19.nocheck.zst` | `zstd -q -f -19 --zstd=wlog=10 --no-check structured_long.bin -o structured_long.w10.l19.nocheck.zst` | 1 | 0 | 1,024 | 18,687 | `91e40b00dcf501d5a5604d22d7f95b7f3db6eb1f3249d0e4056b93892e585dcf` |
| `structured_long.w12.l9.check.zst` | `zstd -q -f -9 --zstd=wlog=12 --check structured_long.bin -o structured_long.w12.l9.check.zst` | 1 | 0 | 4,096 | 15,467 | `91e40b00dcf501d5a5604d22d7f95b7f3db6eb1f3249d0e4056b93892e585dcf` |
| `structured_long.w15.l22.check.zst` | `zstd -q -f --ultra -22 --zstd=wlog=15 --check structured_long.bin -o structured_long.w15.l22.check.zst` | 1 | 0 | 32,768 | 13,031 | `91e40b00dcf501d5a5604d22d7f95b7f3db6eb1f3249d0e4056b93892e585dcf` |
| `far_repeats.l3.check.zst` | `zstd -q -f -3 --check far_repeats.bin -o far_repeats.l3.check.zst` | 1 | 0 | 421,440 | 79,990 | `1e833dbf8c4eda25f82471ab48c888662fc1f2168235bc63bb54b2e2fd92930d` |
| `far_repeats.l19.check.zst` | `zstd -q -f -19 --check far_repeats.bin -o far_repeats.l19.check.zst` | 1 | 0 | 421,440 | 79,048 | `1e833dbf8c4eda25f82471ab48c888662fc1f2168235bc63bb54b2e2fd92930d` |
| `mixed3.frames3.zst` | `zstd -q -f -3 --no-check rle.bin -o frame0.zst; zstd -q -f -9 --check random.bin -o frame1.zst; zstd -q -f -19 --check structured.bin -o frame2.zst; cat frame0.zst frame1.zst frame2.zst > mixed3.frames3.zst` | 3 | 0 | 6,730 | 5,578 | `65dce650b79e1c1778743c6ecd9b1ab32dc1d7e4b9b948adfc22af6a1191baeb` |
| `structured.skippable-first.zst` | `printf '\120\052\115\030\070\000\000\000litegen ktx2 zstd corpus: user data of a skippable frame' > skippable.bin; cat skippable.bin structured.l19.check.zst > structured.skippable-first.zst` | 1 | 1 | 6,730 | 1,513 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |

## The expected outputs

One per input, since every encoding of an input decodes to the same bytes (the script
checks it): the input compressed to fit, as one frame, with `--ultra -22 --long=27
--check`. The test decodes it with `ruzstd` and requires the table's sha256, which is
`zstd -d`'s, so the reference is not taken on `ruzstd`'s word.

| file | command | frames | skippable | window | bytes | sha256 of `zstd -d` |
|---|---|---:|---:|---:|---:|---|
| `uastc_normal.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check uastc_normal.bin -o uastc_normal.expected.zst` | 1 | 0 | 1,048,576 | 378 | `2fbad70bbe4f151de20a425da729ea8d612ce393b5f860d3ad86961b32e8e8cb` |
| `uastc_all_modes.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check uastc_all_modes.bin -o uastc_all_modes.expected.zst` | 1 | 0 | 3,840 | 3,854 | `66069166f4c3f9829e3aad6fbb8c89e81625c51b5602d876e834d7a463ec315e` |
| `rle.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check rle.bin -o rle.expected.zst` | 1 | 0 | 4,096 | 22 | `f600eca824e84a43f0691b267bd620e462c50da165c5b80e17aecb7a924f1fa8` |
| `random.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check random.bin -o random.expected.zst` | 1 | 0 | 4,096 | 4,110 | `5dae7885e079ee784d248b8c2dd0f88dc50c2fa56324d397bf12503a44d43e84` |
| `structured.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check structured.bin -o structured.expected.zst` | 1 | 0 | 6,730 | 1,449 | `61c538b43c0759f7187018e8188da82cf3e84e4e3a660058069eaf1e1730177d` |
| `structured_long.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check structured_long.bin -o structured_long.expected.zst` | 1 | 0 | 73,626 | 12,924 | `91e40b00dcf501d5a5604d22d7f95b7f3db6eb1f3249d0e4056b93892e585dcf` |
| `unit.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check unit.bin -o unit.expected.zst` | 1 | 0 | 3,200 | 2,631 | `2628deebaa286018e909fd9778afbce55b5f350aea1f1a4aaabe39f91767569d` |
| `far_repeats.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check far_repeats.bin -o far_repeats.expected.zst` | 1 | 0 | 421,440 | 79,025 | `1e833dbf8c4eda25f82471ab48c888662fc1f2168235bc63bb54b2e2fd92930d` |
| `mixed3.expected.zst` | `zstd -q -f --ultra -22 --long=27 --check mixed3.bin -o mixed3.expected.zst` | 1 | 0 | 14,922 | 6,028 | `65dce650b79e1c1778743c6ecd9b1ab32dc1d7e4b9b948adfc22af6a1191baeb` |

## What the files hold

`the_corpus_reaches_every_block_kind` reads the bytes of the 89 encodings (91 frames and 1
skippable frame; the largest window is 128 MiB) and counts what is in their blocks (RFC 8878
sections 3.1.1.2 to 3.1.1.3.2). It fails if any cell below is 0, so a corpus made again with
other settings has to keep reaching all of it. Its parser agrees with `zstd -lv` on every
file's frame count, skippable count and window (`the_readme_lists_the_embedded_corpus`).

| blocks | raw | RLE | compressed |
|---|---:|---:|---:|
| count | 43 | 6 | 2,315 |

| literals sections of the compressed blocks | raw | RLE | Huffman-compressed | treeless |
|---|---:|---:|---:|---:|
| count | 2,184 | 1 | 118 | 12 |

| sequence tables | predefined | RLE | FSE compressed | repeat |
|---|---:|---:|---:|---:|
| literal lengths | 2,173 | 5 | 68 | 69 |
| offsets | 2,172 | 10 | 44 | 89 |
| match lengths | 2,242 | 5 | 65 | 3 |

What only the largest input reaches (`far_repeats`; before it the corpus had none of these,
and the test, run against the 95 files without it, failed on the first three):

| | count or size |
|---|---:|
| Huffman literals with size format 3 (18-bit sizes, a header of 5 bytes): compressed, treeless | 2, 0 |
| Huffman trees written as direct 4-bit weights, as FSE-compressed weights | 2, 116 |
| the largest compressed block | 71,795 bytes (64 KiB is 65,536) |
| the farthest back a match reaches (a block is at most 128 KiB) | 567,024 bytes |

The last row is read by a decoder of the sequences that is in the test (it builds the
literal length, offset and match length tables, reads the bitstream backward, and keeps
the three repeat offsets), run on every file with the input it decodes to: each match must
copy the bytes the input holds at that distance, each block's bitstream must end exactly
where its last sequence does, and the literals and matches must make every byte of the
input (29,119 sequences, 18,238,108 bytes). So a wrong offset is a failure of the test, not
a wrong number in this table. The farthest match, by input: `uastc_normal` 567,024 (the two
`--long=27` files; the others 213,616 to 249,808), `far_repeats` 378,682, `structured_long`
69,730, `structured` 5,782, `mixed3` 4,836, `unit` 80, `rle` 1, `uastc_all_modes` and
`random` 0. libzstd's own `ZSTD_generateSequences`, asked what it chooses for the same
inputs at the same levels (`probe_sequences.c` in this folder; not part of the tests),
reports the same farthest offset for every file that does not use the long-distance matcher:
213,616 and 249,808 for `uastc_normal`, 378,682 for `far_repeats`, 69,730 for
`structured_long`. The `--long=27` files of `uastc_normal` are the only ones where the
long-distance matcher took a match farther than the other parsers did, and so the only
ones that reach 567,024. libzstd's probe with the same settings reports 723,248 for them,
a different run of the matcher from the tool's, so it neither confirms nor refutes that
number: it rests on the test's decoder and on its check against the input.

Repeat offsets (an offset value of 1, 2 or 3) are counted the same way, by which one a
match named: the previous offset 12,520 times, the one before it 3,568, the one before that
977, and the previous minus one (the offset value 3 after no literals) twice.

(The tallies are of the blocks, over all the files; most blocks are the tiny ones of
`uastc_normal.w10.*`, which has 1,024 per file.) The rarest cells come from files made for
them: the one RLE literals section from `rle.fast1000.zst`, the RLE tables of the literal
and match lengths from `unit.*`, the treeless literals and repeat tables from
`structured_long.*` and the `w10` files, and the format 3 literals and the direct weights
from `far_repeats.*`. Not reached: a compressed block with no sequences, and treeless
literals with the 5-byte header.


## What litegen takes of this

`ktx2.rs` takes a KTX2 level as exactly one Zstandard frame, from its first byte to its
last (its documentation lists why). So of the files above:

- **a single frame**, with or without a checksum, whichever window it names up to
  `max(level size, 8 MiB)`: decoded by `inflate`, and the bytes must be `zstd -d`'s;
- **`*.stream.l22.long27.zst`** name a 128 MiB window: `inflate` refuses them as
  `Malformed`, by their window (a KTX2 level written by libktx names its own size).
  `ruzstd` itself, with its window limit raised to `2^27`, must decode them to `zstd -d`'s
  bytes;
- **`mixed3.frames3.zst` and `structured.skippable-first.zst`**: `zstd -d` takes them (it
  decodes every frame and skips the skippable one), and `inflate` refuses them as
  `Malformed`: one is more than one frame and the other does not begin with a frame. The
  same streaming API decodes them to `zstd -d`'s bytes frame by frame, recreating the
  decoder for each and skipping the skippable frame by its length, as the `StreamingDecoder`
  documentation says to.

The checksum of a `--check` frame is read (its four bytes belong to the frame) and not
verified: `ruzstd` needs its `hash` feature for that, which brings in a dependency, and it
is off. The bytes are checked against `zstd -d`'s here instead.

## Licence

`uastc_normal` is level 0 of `FlightHelmet_Materials_LensesMat_Normal.ktx2` (CC0 1.0;
see `../README.md`). Everything else here is ours, with no third-party content: the other
inputs are generated, and the `.zst` files are what the `zstd` tool (BSD-licensed) made of
them.

## To make the files again

From the repository root, with `zstd` and node on the `PATH`:

```sh
scratch=$(mktemp -d)
node litegen-core/src/mesh/testdata/ktx2/zstd/make_corpus.mjs "$scratch"
```

It rewrites every `.zst` here, checks each against its input, and writes the tables above
(`$scratch/tables.md`) and the file list the tests embed (`$scratch/files.txt`).
