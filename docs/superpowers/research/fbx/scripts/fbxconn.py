#!/usr/bin/env python3
"""Summarise Connections by (type, child class, parent class, property). Usage: fbxconn.py FILE"""
import struct, sys, zlib, collections
d = open(sys.argv[1], "rb").read()
ver = struct.unpack_from("<I", d, 23)[0]; wide = ver >= 7500; hdr = 25 if wide else 13
objs = {0: ("RootNode", "")}; conns = []
def props(off, n):
    out = []
    for _ in range(n):
        t = chr(d[off]); off += 1
        fx = {"Y": ("<h", 2), "C": ("<B", 1), "I": ("<i", 4), "F": ("<f", 4), "D": ("<d", 8), "L": ("<q", 8)}
        if t in fx:
            f, s = fx[t]; out.append(struct.unpack_from(f, d, off)[0]); off += s
        elif t in "SR":
            n2 = struct.unpack_from("<I", d, off)[0]; out.append(d[off + 4: off + 4 + n2]); off += 4 + n2
        else:
            n2, enc, clen = struct.unpack_from("<III", d, off); out.append(None); off += 12 + clen
    return out, off
def node(off, path):
    if wide: end, np, pl = struct.unpack_from("<QQQ", d, off)
    else: end, np, pl = struct.unpack_from("<III", d, off)
    nl = d[off + hdr - 1]; name = d[off + hdr: off + hdr + nl].decode()
    if end == 0: return None, off + hdr
    p, q = props(off + hdr + nl, np)
    if path == ["Objects"]:
        objs[p[0]] = (name, p[2].decode() if len(p) > 2 else "")
    if path == ["Connections"] and name == "C":
        conns.append(p)
    while q < end:
        c, q = node(q, path + [name])
    return name, end
off = 27
while True:
    n, off = node(off, [])
    if n is None: break
cnt = collections.Counter()
for c in conns:
    typ = c[0].decode(); ch = objs.get(c[1], ("?", "")); pa = objs.get(c[2], ("?", ""))
    prop = c[3].decode() if len(c) > 3 else ""
    cnt[(typ, ch[0] + ":" + ch[1], pa[0] + ":" + pa[1], prop)] += 1
for k, v in sorted(cnt.items(), key=lambda kv: (kv[0][2], kv[0][1])):
    print("%5d  %s  %-32s -> %-32s %s" % (v, k[0], k[1], k[2], k[3]))
