#!/usr/bin/env python3
"""Dump ZIP local headers / central directory with alignment info (stdlib only)."""
import struct, sys

data = open(sys.argv[1], 'rb').read()
print(f'file size {len(data)}')
off = 0
while off + 4 <= len(data) and data[off:off + 4] == b'PK\x03\x04':
    (sig, ver, flags, method, mtime, mdate, crc, csize, usize, nlen, xlen) = struct.unpack_from('<IHHHHHIIIHH', data, off)
    name = data[off + 30:off + 30 + nlen].decode('utf-8', 'replace')
    extra = data[off + 30 + nlen:off + 30 + nlen + xlen]
    xs, i = [], 0
    while i + 4 <= len(extra):
        hid, hlen = struct.unpack_from('<HH', extra, i)
        xs.append(f'id=0x{hid:04x}({hid}) len={hlen}')
        i += 4 + hlen
    doff = off + 30 + nlen + xlen
    print(f'LFH @{off}: ver={ver} flags=0x{flags:04x} method={method} crc=0x{crc:08x} csize={csize} usize={usize} '
          f'name={name!r} extra_len={xlen} [{"; ".join(xs)}] data@{doff} (mod64={doff % 64})')
    off = doff + csize
    if flags & 0x8:
        print('   data descriptor present')
        if data[off:off + 4] == b'PK\x07\x08':
            off += 16
        else:
            off += 12
# central directory
eocd = data.rfind(b'PK\x05\x06')
if eocd >= 0:
    (_, disk, cdisk, n_disk, n_tot, cd_size, cd_off, clen) = struct.unpack_from('<IHHHHIIH', data, eocd)
    print(f'EOCD @{eocd}: entries={n_tot} cd_size={cd_size} cd_off={cd_off} comment_len={clen}')
    print('zip64 EOCD present:', data.find(b'PK\x06\x06') >= 0)
    p = cd_off
    for _ in range(n_tot):
        (sig, vmade, vneed, flags, method, mt, md, crc, cs, us, nl, xl, cl, dn, ia, ea, lho) = struct.unpack_from('<IHHHHHHIIIHHHHHII', data, p)
        name = data[p + 46:p + 46 + nl].decode()
        print(f'  CD: {name!r} made_by=0x{vmade:04x} need={vneed} flags=0x{flags:04x} method={method} '
              f'extra_len={xl} ext_attr=0x{ea:08x} lho={lho}')
        p += 46 + nl + xl + cl
