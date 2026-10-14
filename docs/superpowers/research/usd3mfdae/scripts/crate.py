#!/usr/bin/env python3
"""Minimal independent USDC (crate) reader, stdlib only, for format verification.

Usage: crate.py file.usdc [--raw]
Prints bootstrap/TOC info and every spec with decoded fields.
"""
import struct, sys

# ---------------------------------------------------------------- LZ4 block

def lz4_block(src, out_size):
    dst = bytearray()
    i, n = 0, len(src)
    while i < n:
        tok = src[i]; i += 1
        lit = tok >> 4
        if lit == 15:
            while True:
                b = src[i]; i += 1; lit += b
                if b != 255: break
        dst += src[i:i + lit]; i += lit
        if i >= n: break                      # last sequence: literals only
        off = src[i] | (src[i + 1] << 8); i += 2
        ml = tok & 15
        if ml == 15:
            while True:
                b = src[i]; i += 1; ml += b
                if b != 255: break
        ml += 4
        start = len(dst) - off
        for k in range(ml):                    # overlap-safe copy
            dst.append(dst[start + k])
    assert len(dst) == out_size or out_size is None, (len(dst), out_size)
    return bytes(dst)


def tf_decompress(buf, out_size):
    """TfFastCompression: byte0 == 0 -> one LZ4 block; else N chunks of [i32 size][block]."""
    n = buf[0]
    if n == 0:
        return lz4_block(buf[1:], out_size)
    out, p = bytearray(), 1
    for _ in range(n):
        sz = struct.unpack_from('<i', buf, p)[0]; p += 4
        out += lz4_block(buf[p:p + sz], None); p += sz
    return bytes(out)

# ---------------------------------------------------------------- int coding

def decode_ints(data, count, wide=False):
    """Sdf_IntegerCompression(64) decoding of an already-LZ4-decompressed buffer."""
    if count == 0:
        return []
    fmt_common = '<q' if wide else '<i'
    common = struct.unpack_from(fmt_common, data, 0)[0]
    p_codes = 8 if wide else 4
    p_vints = p_codes + (count * 2 + 7) // 8
    sizes = ({1: '<h', 2: '<i', 3: '<q'} if wide else {1: '<b', 2: '<h', 3: '<i'})
    out, prev = [], 0
    for i in range(count):
        code = (data[p_codes + i // 4] >> (2 * (i % 4))) & 3
        if code == 0:
            d = common
        else:
            f = sizes[code]
            d = struct.unpack_from(f, data, p_vints)[0]
            p_vints += struct.calcsize(f)
        prev += d
        out.append(prev)
    return out


class R:
    def __init__(self, data):
        self.d, self.p = data, 0
    def u8(self):
        v = self.d[self.p]; self.p += 1; return v
    def un(self, fmt):
        v = struct.unpack_from(fmt, self.d, self.p); self.p += struct.calcsize(fmt)
        return v if len(v) > 1 else v[0]
    def bytes(self, n):
        v = self.d[self.p:self.p + n]; self.p += n; return v


def read_comp_ints(r, count, wide=False):
    csize = r.un('<Q')
    comp = r.bytes(csize)
    enc_size = (8 if wide else 4) + (count * 2 + 7) // 8 + count * (8 if wide else 4)
    raw = tf_decompress(comp, None)
    assert len(raw) <= enc_size
    return decode_ints(raw, count, wide)

# ---------------------------------------------------------------- type tables

T = dict(Invalid=0, Bool=1, UChar=2, Int=3, UInt=4, Int64=5, UInt64=6, Half=7, Float=8,
         Double=9, String=10, Token=11, AssetPath=12, Matrix2d=13, Matrix3d=14, Matrix4d=15,
         Quatd=16, Quatf=17, Quath=18, Vec2d=19, Vec2f=20, Vec2h=21, Vec2i=22, Vec3d=23,
         Vec3f=24, Vec3h=25, Vec3i=26, Vec4d=27, Vec4f=28, Vec4h=29, Vec4i=30, Dictionary=31,
         TokenListOp=32, StringListOp=33, PathListOp=34, ReferenceListOp=35, IntListOp=36,
         Int64ListOp=37, UIntListOp=38, UInt64ListOp=39, PathVector=40, TokenVector=41,
         Specifier=42, Permission=43, Variability=44, VariantSelectionMap=45, TimeSamples=46,
         Payload=47, DoubleVector=48, LayerOffsetVector=49, StringVector=50, ValueBlock=51,
         Value=52, UnregisteredValue=53, UnregisteredValueListOp=54, PayloadListOp=55,
         TimeCode=56, PathExpression=57, Relocates=58, Spline=59, AnimationBlock=60)
TN = {v: k for k, v in T.items()}
# element struct formats for bitwise POD types
POD = {1: '<?', 2: '<B', 3: '<i', 4: '<I', 5: '<q', 6: '<Q', 7: '<e', 8: '<f', 9: '<d',
       13: '<4d', 14: '<9d', 15: '<16d', 16: '<4d', 17: '<4f', 18: '<4e',
       19: '<2d', 20: '<2f', 21: '<2e', 22: '<2i', 23: '<3d', 24: '<3f', 25: '<3e', 26: '<3i',
       27: '<4d', 28: '<4f', 29: '<4e', 30: '<4i', 56: '<d'}
VEC_DIM = {19: 2, 20: 2, 21: 2, 22: 2, 23: 3, 24: 3, 25: 3, 26: 3, 27: 4, 28: 4, 29: 4, 30: 4,
           16: 4, 17: 4, 18: 4}
MAT_DIM = {13: 2, 14: 3, 15: 4}
SPEC = {0: 'Unknown', 1: 'Attribute', 2: 'Connection', 3: 'Expression', 4: 'Mapper',
        5: 'MapperArg', 6: 'Prim', 7: 'PseudoRoot', 8: 'Relationship',
        9: 'RelationshipTarget', 10: 'Variant', 11: 'VariantSet'}


class Crate:
    def __init__(self, data):
        self.d = data
        r = R(data)
        assert r.bytes(8) == b'PXR-USDC', 'bad magic'
        self.ver = tuple(r.bytes(8)[:3])
        self.toc_off = r.un('<q')
        r.p = self.toc_off
        n = r.un('<Q')
        self.sections = {}
        for _ in range(n):
            name = r.bytes(16).split(b'\0')[0].decode()
            start, size = r.un('<qq')
            self.sections[name] = (start, size)
        assert self.ver >= (0, 4, 0), 'only >=0.4.0 implemented here'
        self._tokens(); self._strings(); self._fields(); self._fieldsets(); self._paths(); self._specs()

    def sec(self, name):
        r = R(self.d); r.p = self.sections[name][0]; return r

    def _tokens(self):
        r = self.sec('TOKENS')
        n = r.un('<Q'); usize = r.un('<Q'); csize = r.un('<Q')
        raw = tf_decompress(r.bytes(csize), usize)
        self.tokens = raw.split(b'\0')[:n]
        self.tokens = [t.decode() for t in self.tokens]

    def _strings(self):
        r = self.sec('STRINGS')
        n = r.un('<Q')
        self.strings = [r.un('<I') for _ in range(n)]   # each = token index

    def _fields(self):
        r = self.sec('FIELDS')
        n = r.un('<Q')
        toks = read_comp_ints(r, n)
        rsize = r.un('<Q')
        reps = tf_decompress(r.bytes(rsize), n * 8)
        self.fields = [(toks[i], struct.unpack_from('<Q', reps, i * 8)[0]) for i in range(n)]

    def _fieldsets(self):
        r = self.sec('FIELDSETS')
        n = r.un('<Q')
        self.fieldsets = [x & 0xffffffff for x in read_comp_ints(r, n)]

    def _paths(self):
        r = self.sec('PATHS')
        total = r.un('<Q')
        n = r.un('<Q')
        idx = read_comp_ints(r, n); elem = read_comp_ints(r, n); jumps = read_comp_ints(r, n)
        self.paths = [None] * total
        self.path_jumps = list(zip(idx, elem, jumps))

        def build(cur, parent):
            while True:
                this = cur; cur += 1
                if parent is None:
                    parent = '/'; self.paths[idx[this]] = '/'
                else:
                    e = elem[this]; tok = self.tokens[abs(e)]
                    if e < 0:
                        p = parent + '.' + tok
                    else:
                        p = ('/' if parent == '/' else parent + '/') + tok
                    self.paths[idx[this]] = p
                j = jumps[this]
                has_child, has_sib = (j > 0 or j == -1), (j >= 0)
                if has_child:
                    if has_sib:
                        build(this + j, parent)
                    parent = self.paths[idx[this]]
                if not (has_child or has_sib):
                    return
        build(0, None)

    def _specs(self):
        r = self.sec('SPECS')
        n = r.un('<Q')
        pi = read_comp_ints(r, n); fs = read_comp_ints(r, n); st = read_comp_ints(r, n)
        self.specs = list(zip(pi, fs, st))

    # ------------------------------------------------------------ values
    def rep_info(self, rep):
        return dict(type=TN.get((rep >> 48) & 0xff, (rep >> 48) & 0xff), array=bool(rep >> 63 & 1),
                    inlined=bool(rep >> 62 & 1), compressed=bool(rep >> 61 & 1),
                    arrayedit=bool(rep >> 60 & 1), payload=rep & ((1 << 48) - 1))

    def value(self, rep):
        t = (rep >> 48) & 0xff
        is_arr, inl, comp = rep >> 63 & 1, rep >> 62 & 1, rep >> 61 & 1
        payload = rep & ((1 << 48) - 1)
        if t == T['TimeSamples']:
            return self.timesamples(payload)
        if is_arr:
            return self.array(t, payload, comp)
        if inl:
            return self.inlined(t, payload & 0xffffffff)
        r = R(self.d); r.p = payload
        return self.read_scalar(r, t)

    def inlined(self, t, v):
        b = struct.pack('<I', v)
        if t == T['Bool']: return bool(v)
        if t in (T['UChar'],): return v & 0xff
        if t == T['Int']: return struct.unpack('<i', b)[0]
        if t == T['UInt']: return v
        if t in (T['Float'], T['Double']): return struct.unpack('<f', b)[0]   # double inlined as float
        if t == T['Half']: return struct.unpack('<e', b[:2])[0]
        if t == T['Token']: return self.tokens[v]
        if t == T['String']: return self.tokens[self.strings[v]]
        if t == T['AssetPath']: return '@' + self.tokens[v] + '@'
        if t == T['Specifier']: return ['def', 'over', 'class'][v]
        if t == T['Variability']: return ['varying', 'uniform', 'uniform(config)'][v]
        if t == T['Permission']: return ['public', 'private'][v]
        if t == T['Vec2h']: return list(struct.unpack('<2e', b))     # 4-byte POD: raw
        if t in VEC_DIM: return list(struct.unpack('<%db' % VEC_DIM[t], b[:VEC_DIM[t]]))
        if t in MAT_DIM:
            n = MAT_DIM[t]; dg = struct.unpack('<%db' % n, b[:n])
            return [[dg[i] if i == j else 0 for j in range(n)] for i in range(n)]
        if t in (T['Dictionary'],): return {}
        if t == T['ValueBlock']: return 'None(ValueBlock)'
        return ('INLINE', TN.get(t), v)

    def read_scalar(self, r, t):
        if t in POD:
            v = r.un(POD[t])
            if t in MAT_DIM:
                n = MAT_DIM[t]; return [list(v[i * n:(i + 1) * n]) for i in range(n)]
            return v
        if t in (T['Token'],): return self.tokens[r.un('<I')]
        if t in (T['String'],): return self.tokens[self.strings[r.un('<I')]]
        if t == T['AssetPath']: return '@' + self.tokens[self.strings[r.un('<I')]] + '@'
        if t == T['TokenVector']:
            n = r.un('<Q'); return [self.tokens[r.un('<I')] for _ in range(n)]
        if t == T['PathVector']:
            n = r.un('<Q'); return [self.paths[r.un('<I')] for _ in range(n)]
        if t == T['DoubleVector']:
            n = r.un('<Q'); return [r.un('<d') for _ in range(n)]
        if t == T['StringVector']:
            n = r.un('<Q'); return [self.tokens[self.strings[r.un('<I')]] for _ in range(n)]
        if t in (T['TokenListOp'], T['PathListOp'], T['StringListOp'], T['IntListOp'],
                 T['ReferenceListOp'], T['PayloadListOp']):
            return self.listop(r, t)
        if t in (T['Dictionary'], T['VariantSelectionMap']):
            n = r.un('<Q'); out = {}
            for _ in range(n):
                k = self.tokens[self.strings[r.un('<I')]]
                if t == T['VariantSelectionMap']:
                    out[k] = self.tokens[self.strings[r.un('<I')]]
                else:
                    # VtValue = [i64 off][nested data][ValueRep]; continue after rep
                    start = r.p; off = r.un('<q'); r.p = start + off
                    out[k] = self.value(r.un('<Q'))
            return out
        if t == T['Payload']:
            a = self.tokens[self.strings[r.un('<I')]]; p = self.paths[r.un('<I')]
            lo = r.un('<dd') if self.ver >= (0, 8, 0) else None
            return dict(assetPath=a, primPath=p, layerOffset=lo)
        return ('UNHANDLED', TN.get(t))

    def listop(self, r, t):
        h = r.u8()
        names = ['explicit', 'add', 'prepend', 'append', 'delete', 'reorder']
        bits = [1 << 1, 1 << 2, 1 << 5, 1 << 6, 1 << 3, 1 << 4]
        out = {'isExplicit': bool(h & 1)} if h & 1 else {}
        for nm, bit in zip(names, bits):
            if h & bit:
                n = r.un('<Q'); items = []
                for _ in range(n):
                    if t == T['TokenListOp']: items.append(self.tokens[r.un('<I')])
                    elif t == T['PathListOp']: items.append(self.paths[r.un('<I')])
                    elif t == T['StringListOp']: items.append(self.tokens[self.strings[r.un('<I')]])
                    elif t == T['IntListOp']: items.append(r.un('<i'))
                    elif t == T['ReferenceListOp']:
                        a = self.tokens[self.strings[r.un('<I')]]; p = self.paths[r.un('<I')]
                        lo = r.un('<dd')
                        n2 = r.un('<Q')       # customData dict (assume empty)
                        assert n2 == 0
                        items.append((a, p, lo))
                    elif t == T['PayloadListOp']:
                        a = self.tokens[self.strings[r.un('<I')]]; p = self.paths[r.un('<I')]
                        items.append((a, p, r.un('<dd')))
                out[nm] = items
        return out

    def array(self, t, payload, comp):
        if payload == 0:
            return []
        r = R(self.d); r.p = payload
        n = r.un('<I') if self.ver < (0, 7, 0) else r.un('<Q')
        if comp and t in (T['Int'], T['UInt'], T['Int64'], T['UInt64']):
            return read_comp_ints(r, n, wide=t in (T['Int64'], T['UInt64'])) if n >= 16 else \
                [r.un(POD[t]) for _ in range(n)]
        if comp and t in (T['Half'], T['Float'], T['Double']):
            if n < 16:
                return [r.un(POD[t]) for _ in range(n)]
            code = chr(r.u8())
            if code == 'i':
                return [float(x) for x in read_comp_ints(r, n)], 'i'
            if code == 't':
                lut_n = r.un('<I'); lut = [r.un(POD[t]) for _ in range(lut_n)]
                ids = read_comp_ints(r, n)
                return [lut[i] for i in ids], 't(lut=%d)' % lut_n
            raise ValueError('bad float code')
        if t in POD:
            f = POD[t]; vals = [r.un(f) for _ in range(n)]
            if t in MAT_DIM:
                k = MAT_DIM[t]; vals = [[list(m[i * k:(i + 1) * k]) for i in range(k)] for m in vals]
            return vals
        if t == T['Token']: return [self.tokens[r.un('<I')] for _ in range(n)]
        if t == T['String']: return [self.tokens[self.strings[r.un('<I')]] for _ in range(n)]
        if t == T['AssetPath']: return ['@%s@' % self.tokens[self.strings[r.un('<I')]] for _ in range(n)]
        return ('UNHANDLED-ARRAY', TN.get(t), n)

    def timesamples(self, payload):
        r = R(self.d); r.p = payload
        start = r.p; off = r.un('<q'); r.p = start + off          # _RecursiveRead
        times_rep = r.un('<Q')
        times = self.value(times_rep)
        start = r.p; off = r.un('<q'); r.p = start + off          # _RecursiveRead
        n = r.un('<Q')
        reps = [r.un('<Q') for _ in range(n)]
        return {'times': times, 'times_rep': self.rep_info(times_rep),
                'values': [self.value(x) for x in reps]}


def main():
    data = open(sys.argv[1], 'rb').read()
    c = Crate(data)
    print('version', '.'.join(map(str, c.ver)), 'tocOffset', c.toc_off, 'fileSize', len(data))
    for k, (s, z) in c.sections.items():
        print(f'  section {k:10s} start={s} size={z}')
    print('tokens(%d):' % len(c.tokens), c.tokens)
    print('strings ->', [c.tokens[i] for i in c.strings])
    print('paths:', c.paths)
    if '--raw' in sys.argv:
        print('path (index, elemToken, jump):', c.path_jumps)
        print('fieldsets:', c.fieldsets)
    for pi, fsi, st in c.specs:
        print(f'SPEC {c.paths[pi]}  [{SPEC.get(st, st)}]')
        i = fsi
        while c.fieldsets[i] != 0xffffffff:
            tok, rep = c.fields[c.fieldsets[i]]
            info = c.rep_info(rep)
            flags = ''.join(f for f, b in (('A', info['array']), ('I', info['inlined']),
                                           ('C', info['compressed'])) if b)
            print(f'    {c.tokens[tok]:28s} <{info["type"]}{"[" + flags + "]" if flags else ""}> = {c.value(rep)!r}')
            i += 1


if __name__ == '__main__':
    main()
