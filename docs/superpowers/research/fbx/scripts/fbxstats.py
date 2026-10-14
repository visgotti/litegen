#!/usr/bin/env python3
"""Array-compression and NULL-record statistics for a binary FBX file. Usage: fbxstats.py FILE"""
import struct, sys, zlib, collections
d = open(sys.argv[1], "rb").read()
ver = struct.unpack_from("<I", d, 23)[0]
wide = ver >= 7500; hdr = 25 if wide else 13
arrs = []  # (type, n, rawbytes, enc, clen, zhdr, nodename)
leaf0_nonull = collections.Counter(); leaf0_null = collections.Counter(); propnull = collections.Counter()
def prop(off, nodename):
    t = chr(d[off]); off += 1
    fixed = {"Y": 2, "C": 1, "I": 4, "F": 4, "D": 8, "L": 8}
    if t in fixed: return off + fixed[t]
    if t in "SR": return off + 4 + struct.unpack_from("<I", d, off)[0]
    n, enc, clen = struct.unpack_from("<III", d, off)
    size = {"f": 4, "d": 8, "l": 8, "i": 4, "b": 1, "c": 1}[t]
    arrs.append((t, n, n * size, enc, clen, d[off + 12: off + 14].hex() if enc else "-", nodename))
    return off + 12 + clen
def node(off):
    if wide: end, nprops, plen = struct.unpack_from("<QQQ", d, off)
    else: end, nprops, plen = struct.unpack_from("<III", d, off)
    nlen = d[off + hdr - 1]; name = d[off + hdr: off + hdr + nlen].decode("latin1")
    if end == 0: return None, off + hdr
    p = off + hdr + nlen
    for _ in range(nprops): p = prop(p, name)
    nchild = 0; has_null = False
    while p < end:
        c, p = node(p)
        if c is None: has_null = True
        else: nchild += 1
    if nchild == 0 and nprops == 0: (leaf0_null if has_null else leaf0_nonull)[name] += 1
    if nchild == 0 and nprops > 0 and has_null: propnull[name] += 1
    return name, end
off = 27
while True:
    n, off = node(off)
    if n is None: break
print("version", ver)
print("leaf nodes with 0 props and NULL:", dict(leaf0_null))
print("leaf nodes with 0 props, no NULL:", dict(leaf0_nonull))
print("leaf nodes with props and NULL:", dict(propnull))
comp = [a for a in arrs if a[3] == 1]; raw = [a for a in arrs if a[3] == 0]
print("arrays: %d compressed, %d raw" % (len(comp), len(raw)))
print("zlib header bytes seen:", collections.Counter(a[5] for a in comp))
if raw: print("largest RAW array bytes:", max(a[2] for a in raw), "; smallest COMPRESSED array bytes:", min(a[2] for a in comp) if comp else None)
print("raw arrays by (node,type,n) sample:", sorted(set((a[6], a[0], a[1]) for a in raw), key=lambda x: -x[2])[:6])
print("compressed small sample:", sorted(set((a[6], a[0], a[1], a[2], a[4]) for a in comp), key=lambda x: x[3])[:6])
print("array types:", collections.Counter(a[0] for a in arrs))
