#!/usr/bin/env python3
"""Tally binary type codes of Properties70 P-record values per declared FBX type. Usage: fbxptypes.py FILE..."""
import struct, sys, zlib, collections
tally = collections.Counter(); other = collections.Counter()
for path in sys.argv[1:]:
    d = open(path, "rb").read()
    ver = struct.unpack_from("<I", d, 23)[0]; wide = ver >= 7500; hdr = 25 if wide else 13
    def props(off, n):
        out = []
        for _ in range(n):
            t = chr(d[off]); off += 1
            sz = {"Y": 2, "C": 1, "I": 4, "F": 4, "D": 8, "L": 8}
            if t in sz: out.append((t, d[off:off + sz[t]])); off += sz[t]
            elif t in "SR":
                k = struct.unpack_from("<I", d, off)[0]; out.append((t, d[off + 4: off + 4 + k])); off += 4 + k
            else:
                k, e, c = struct.unpack_from("<III", d, off); out.append((t, None)); off += 12 + c
        return out, off
    def node(off, parent):
        if wide: end, np_, pl = struct.unpack_from("<QQQ", d, off)
        else: end, np_, pl = struct.unpack_from("<III", d, off)
        nl = d[off + hdr - 1]; name = d[off + hdr: off + hdr + nl].decode()
        if end == 0: return None, off + hdr
        p, q = props(off + hdr + nl, np_)
        if name == "P" and parent == "Properties70":
            decl = p[1][1].decode(errors="replace")
            codes = "".join(t for t, _ in p[4:])
            tally[(decl, codes)] += 1
        elif not parent in ("Properties70",) and np_ and name not in ("P",):
            other[(name, "".join(t for t, _ in p))] += 1
        while q < end:
            c, q = node(q, name)
        return name, end
    off = 27
    while True:
        n, off = node(off, "")
        if n is None: break
for k, v in sorted(tally.items()):
    print("%-28s %-6s %d" % (k[0], k[1] or "(none)", v))
print("---- non-P node property signatures (sample) ----")
for k, v in sorted(other.items())[:400]:
    print("%-28s %-8s %d" % (k[0], k[1], v))
