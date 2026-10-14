#!/usr/bin/env python3
"""Minimal binary FBX dumper used to verify container details.

Usage: fbxdump.py FILE [--tree] [--maxdepth N]
Prints header, every node with offsets, whether it ends with a NULL record,
array encodings, and a byte-exact breakdown of the footer.
"""
import struct, sys, zlib

path = sys.argv[1]
show_tree = "--tree" in sys.argv
maxdepth = 99
if "--maxdepth" in sys.argv:
    maxdepth = int(sys.argv[sys.argv.index("--maxdepth") + 1])
d = open(path, "rb").read()

magic = d[:23]
print("magic   :", magic)
ver = struct.unpack_from("<I", d, 23)[0]
print("version :", ver)
wide = ver >= 7500
hdr = 25 if wide else 13
stats = {"arrays": 0, "compressed": 0, "nodes": 0, "null_after_children": 0,
         "null_without_children": [], "no_null_with_children": []}


def prop(off):
    t = chr(d[off]); off += 1
    if t == "Y": return t, struct.unpack_from("<h", d, off)[0], off + 2
    if t == "C": return t, d[off], off + 1
    if t == "I": return t, struct.unpack_from("<i", d, off)[0], off + 4
    if t == "F": return t, struct.unpack_from("<f", d, off)[0], off + 4
    if t == "D": return t, struct.unpack_from("<d", d, off)[0], off + 8
    if t == "L": return t, struct.unpack_from("<q", d, off)[0], off + 8
    if t in "SR":
        n = struct.unpack_from("<I", d, off)[0]
        v = d[off + 4: off + 4 + n]
        return t, v, off + 4 + n
    if t in "fdlibc":
        n, enc, clen = struct.unpack_from("<III", d, off)
        raw = d[off + 12: off + 12 + clen]
        stats["arrays"] += 1
        if enc == 1:
            stats["compressed"] += 1
            raw2 = zlib.decompress(raw)
            hdrb = raw[:2].hex()
        else:
            raw2 = raw; hdrb = "-"
        size = {"f": 4, "d": 8, "l": 8, "i": 4, "b": 1, "c": 1}[t]
        assert len(raw2) == n * size, (t, n, len(raw2))
        fmt = {"f": "f", "d": "d", "l": "q", "i": "i", "b": "B", "c": "B"}[t]
        vals = struct.unpack("<%d%s" % (n, fmt), raw2)
        return t, ("arr n=%d enc=%d clen=%d zhdr=%s" % (n, enc, clen, hdrb), vals[:8]), off + 12 + clen
    raise ValueError("bad type %r at %d" % (t, off - 1))


def node(off, depth):
    if wide:
        end, nprops, plen = struct.unpack_from("<QQQ", d, off)
    else:
        end, nprops, plen = struct.unpack_from("<III", d, off)
    nlen = d[off + hdr - 1]
    name = d[off + hdr: off + hdr + nlen].decode("latin1")
    if end == 0:
        return None, off + hdr
    stats["nodes"] += 1
    p = off + hdr + nlen
    props = []
    for _ in range(nprops):
        t, v, p = prop(p)
        props.append((t, v))
    assert p - (off + hdr + nlen) == plen, ("plen mismatch", name)
    children = []
    has_null = False
    while p < end:
        c, p2 = node(p, depth + 1)
        if c is None:
            has_null = True
            assert p2 == end, ("NULL record not last", name, p2, end)
        else:
            children.append(c)
        p = p2
    assert p == end, (name, p, end)
    if children and has_null:
        stats["null_after_children"] += 1
    if not children and has_null:
        stats["null_without_children"].append((name, len(props)))
    if children and not has_null:
        stats["no_null_with_children"].append(name)
    if show_tree and depth <= maxdepth:
        def fmtp(t, v):
            if t in "SR":
                return t + ":" + repr(v[:60])
            return t + ":" + repr(v)[:110]
        print("  " * depth + "%s @%d end=%d props=[%s]%s" % (
            name, off, end, ", ".join(fmtp(t, v) for t, v in props),
            " +NULL" if has_null else ""))
    return (name, props, children), end


off = 27
top = []
while True:
    n, off2 = node(off, 0)
    if n is None:
        print("top-level NULL record at", off, "len", off2 - off)
        off = off2
        break
    top.append(n[0])
    off = off2
print("top-level nodes:", top)
print("stats:", {k: v for k, v in stats.items()})
foot = d[off:]
print("footer starts at", off, "length", len(foot), "file size", len(d), "file size %16 =", len(d) % 16)
print("footer[0:16]  (footer id):", foot[:16].hex())
rest = foot[16:]
# find version position: after 4 zero bytes + pad
print("bytes after footer id (hex, first 48):", rest[:48].hex())
# Locate version
vpos = None
for i in range(0, 40):
    if struct.unpack_from("<I", rest, i)[0] == ver:
        vpos = i
        break
print("version found at footer offset", 16 + vpos if vpos is not None else None,
      "-> zero bytes between footer id and version:", vpos,
      "(absolute file offset of version: %d, %%16=%d)" % (off + 16 + vpos, (off + 16 + vpos) % 16) if vpos is not None else "")
if vpos is not None:
    after = rest[vpos + 4:]
    zeros = len(after) - 16
    print("bytes after version:", len(after), "zeros-before-final-magic:", zeros,
          "all zero:", all(b == 0 for b in after[:zeros]))
    print("final 16 bytes:", after[-16:].hex())
