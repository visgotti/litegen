#!/usr/bin/env python3
"""Rewrite a binary_little_endian PLY file as binary_big_endian, value for value.

    python3 -I make_be.py <in.ply> <out.ply>

It shares nothing with litegen: it parses the header itself, reads each value with
an explicit little-endian struct format ('<') and writes it with an explicit
big-endian one ('>'), list counts included. A header line it does not understand,
or a body that ends early or runs on, stops it before it writes. See README.md.
"""
import struct
import sys

# PLY's eight scalar types under both spellings, as struct codes. A list's count
# must be one of the integer types.
CODE = {'char': 'b', 'int8': 'b', 'uchar': 'B', 'uint8': 'B',
        'short': 'h', 'int16': 'h', 'ushort': 'H', 'uint16': 'H',
        'int': 'i', 'int32': 'i', 'uint': 'I', 'uint32': 'I',
        'float': 'f', 'float32': 'f', 'double': 'd', 'float64': 'd'}
COUNT = {name: code for name, code in CODE.items() if code not in 'fd'}


def fail(why):
    sys.exit(f'make_be.py: {why}; nothing written')


if len(sys.argv) != 3:
    fail('usage: python3 -I make_be.py <in.ply> <out.ply>')
with open(sys.argv[1], 'rb') as f:
    data = f.read()

# The header, a line at a time. Only the format line is rewritten.
lines, at = [], 0
while lines[-1:] != ['end_header']:
    end = data.find(b'\n', at)
    if end < 0:
        fail('the header has no end_header line')
    lines.append(data[at:end].decode('ascii'))
    at = end + 1
if lines[:2] != ['ply', 'format binary_little_endian 1.0']:
    fail(f'not a binary_little_endian 1.0 PLY file: {lines[:2]}')
elements = []  # (record count, [(list count code or None, value code)]), in header order
for line in lines[2:-1]:
    w = line.split()
    if w[:1] == ['element'] and len(w) == 3 and w[2].isdigit():
        elements.append((int(w[2]), []))
    elif w[:2] == ['property', 'list'] and len(w) == 5 and elements and w[2] in COUNT and w[3] in CODE:
        elements[-1][1].append((COUNT[w[2]], CODE[w[3]]))
    elif w[:1] == ['property'] and len(w) == 3 and elements and w[1] in CODE:
        elements[-1][1].append((None, CODE[w[1]]))
    elif w[:1] not in (['comment'], ['obj_info']):
        fail(f'header line {line!r} not understood')
out = [('\n'.join(['ply', 'format binary_big_endian 1.0'] + lines[2:]) + '\n').encode('ascii')]


def take(code):
    """The body's next value, read little-endian."""
    global at
    size = struct.calcsize('<' + code)
    if at + size > len(data):
        fail(f'the body ends early, at byte {len(data)}')
    (value,) = struct.unpack_from('<' + code, data, at)
    at += size
    return value


# The body: every record of every element, in header order.
for records, properties in elements:
    for _ in range(records):
        for count_code, code in properties:
            if count_code is None:  # a scalar
                out.append(struct.pack('>' + code, take(code)))
                continue
            n = take(count_code)  # a list: its count, then that many values
            if n < 0:
                fail(f'a negative list count, before byte {at}')
            out.append(struct.pack('>' + count_code, n))
            out.extend(struct.pack('>' + code, take(code)) for _ in range(n))
if at != len(data):
    fail(f'{len(data) - at} bytes follow the last element')
with open(sys.argv[2], 'wb') as f:
    f.write(b''.join(out))
