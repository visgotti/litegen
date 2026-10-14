#!/usr/bin/env python3
"""Print an FBX binary file as an ASCII-like tree.
Usage: fbxtree.py FILE [MAXARR] [FILTER_TOPLEVEL_OBJECT_CLASS ...]
Arrays are truncated to MAXARR values (default 12)."""
import struct, sys, zlib
d = open(sys.argv[1], "rb").read()
maxarr = int(sys.argv[2]) if len(sys.argv) > 2 else 12
filt = set(sys.argv[3:])
ver = struct.unpack_from("<I", d, 23)[0]
wide = ver >= 7500; hdr = 25 if wide else 13
def prop(off):
    t = chr(d[off]); off += 1
    fx = {"Y": ("<h", 2), "C": ("<B", 1), "I": ("<i", 4), "F": ("<f", 4), "D": ("<d", 8), "L": ("<q", 8)}
    if t in fx:
        f, s = fx[t]; v = struct.unpack_from(f, d, off)[0]
        if t == "C": v = "C:%r" % chr(v)
        return v, off + s
    if t == "S":
        n = struct.unpack_from("<I", d, off)[0]; v = d[off + 4: off + 4 + n]
        if b"\x00\x01" in v:
            a, b = v.split(b"\x00\x01", 1); v = (b + b"::" + a)
        return '"%s"' % v.decode("utf8", "replace"), off + 4 + n
    if t == "R":
        n = struct.unpack_from("<I", d, off)[0]; v = d[off + 4: off + 4 + n]
        return "R[%d]:%s" % (n, v[:24].hex()), off + 4 + n
    n, enc, clen = struct.unpack_from("<III", d, off)
    raw = d[off + 12: off + 12 + clen]
    if enc: raw = zlib.decompress(raw)
    fmt = {"f": "f", "d": "d", "l": "q", "i": "i", "b": "B", "c": "B"}[t]
    vals = struct.unpack("<%d%s" % (n, fmt), raw)
    def f(x): return ("%.9g" % x) if isinstance(x, float) else str(x)
    s = ",".join(f(x) for x in vals[:maxarr]) + (",..." if n > maxarr else "")
    return "%s*%d {%s}" % (t, n, s), off + 12 + clen
def node(off, depth, out):
    if wide: end, nprops, plen = struct.unpack_from("<QQQ", d, off)
    else: end, nprops, plen = struct.unpack_from("<III", d, off)
    nlen = d[off + hdr - 1]; name = d[off + hdr: off + hdr + nlen].decode("latin1")
    if end == 0: return None, off + hdr
    p = off + hdr + nlen; props = []
    for _ in range(nprops):
        v, p = prop(p); props.append(str(v))
    lines = ["\t" * depth + name + ": " + ", ".join(props)]
    kids = []
    while p < end:
        c, p = node(p, depth + 1, out)
        if c is not None: kids.append(c)
    if kids:
        lines[0] += " {"
        for k in kids: lines.extend(k[1])
        lines.append("\t" * depth + "}")
    return (name, lines), end
off = 27
while True:
    n, off = node(off, 0, None)
    if n is None: break
    name, lines = n
    if not filt:
        print("\n".join(lines))
    elif name == "Objects":
        # print only objects whose node name is in filt
        cur = None; buf = []
        for ln in lines[1:-1]:
            if ln.startswith("\t") and not ln.startswith("\t\t"):
                if cur and buf: print("\n".join(buf))
                cur = ln.strip().split(":")[0]; buf = [ln] if cur in filt else []
            elif buf: buf.append(ln)
        if buf: print("\n".join(buf))
    elif name in filt:
        print("\n".join(lines))
