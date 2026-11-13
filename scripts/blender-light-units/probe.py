# Blender light-unit probe for litegen's `light-units` work (Task 1; rerun on Task 5's new files).
#
# Run it headless in a Blender bundle that belongs to somebody else's project, so it must write
# nothing there:
#
#   touch "$OUT/marker"
#   PYTHONDONTWRITEBYTECODE=1 "$BLENDER" -b --factory-startup --python-use-system-env \
#       --python scripts/blender-light-units/probe.py -- \
#       --out "$OUT" --glb ref.glb --fbx a.fbx --fbx b.fbx --usd a.usda --usd b.usdz --synthetic --predict
#   find "$BLENDER_APP" -newer "$OUT/marker" -type f      # must print nothing
#
# What it does, in one run (Blender 5.2):
#   1. imports each --glb through bpy.ops.import_scene.gltf (default lighting mode, recorded);
#   2. imports each --fbx through wm.fbx_import (C++ ufbx, the default) and import_scene.fbx
#      (Python, "Legacy"; in 5.2.0 it raises on every light, so it is run twice: as shipped, where
#      only the orphan Light datablock's energy is readable, and with a Cycles cast_shadow stub),
#      and each --usd through wm.usd_import;
#      every input is copied under OUT/inputs/ first, so Blender never writes beside the originals;
#   3. for the first --glb: exports the glTF-imported scene with export_scene.fbx and wm.usd_export
#      (usda), reads the raw light values out of those files, and re-imports them;
#   4. --synthetic: authors Blender lights of known energy, exposure, radius and normalize,
#      exports them (FBX, USD), reads the raw values, re-imports them; and imports hand-written
#      UsdLux files that vary intensity, exposure, normalize, radius, angle and the import's
#      light_intensity_scale, to see which inputs reach `energy`.
#   5. --predict (needs --glb): copies the first --fbx / first .usda --usd, overwrites each light's
#      Intensity with what `blender` units would write (FBX 100*E; USD E/pi, sun E/4, E = the
#      glTF-import energy), and imports them;
#   6. prints one table per step and the derived factors, and writes OUT/readings.json.
#
# Raw values come from the files themselves (own binary-FBX and USDA parsers below), so the
# factor is "raw file value" / "energy Blender reports", not an assumption about the importer.
import sys
sys.dont_write_bytecode = True

import argparse
import json
import math
import os
import re
import shutil
import struct
import zipfile
import zlib

import bpy

KIND_ORDER = {"SUN": 0, "POINT": 1, "SPOT": 2}


# ---------------------------------------------------------------------------------------------
# Raw file readers (no Blender involved)
# ---------------------------------------------------------------------------------------------

def fbx_parse(data):
    """Minimal binary-FBX reader: (version, [(name, props, children, offsets)]) for the top-level records.

    `offsets[i]` is the file offset of property i's payload (after its type byte), so a caller can
    patch a scalar in place.
    """
    assert data[:20] == b"Kaydara FBX Binary  ", "not a binary FBX"
    version = struct.unpack_from("<I", data, 23)[0]
    big = version >= 7500
    hdr = 25 if big else 13
    fmt = "<QQQB" if big else "<IIIB"
    sizes = {"Y": 2, "C": 1, "I": 4, "F": 4, "D": 8, "L": 8}
    arr_sizes = {"f": 4, "d": 8, "l": 8, "i": 4, "b": 1}

    def read_node(pos):
        end, nprops, plen, nlen = struct.unpack_from(fmt, data, pos)
        if end == 0:
            return None, pos + hdr
        name = data[pos + hdr:pos + hdr + nlen].decode("latin1")
        p = pos + hdr + nlen
        props = []
        offs = []
        for _ in range(nprops):
            t = chr(data[p])
            p += 1
            offs.append((t, p))
            if t in ("S", "R"):
                (ln,) = struct.unpack_from("<I", data, p)
                raw = data[p + 4:p + 4 + ln]
                p += 4 + ln
                props.append(raw.decode("utf-8", "replace") if t == "S" else raw)
            elif t in sizes:
                code = {"Y": "<h", "C": "<B", "I": "<i", "F": "<f", "D": "<d", "L": "<q"}[t]
                props.append(struct.unpack_from(code, data, p)[0])
                p += sizes[t]
            elif t in arr_sizes:
                alen, enc, clen = struct.unpack_from("<III", data, p)
                p += 12
                p += clen if enc else alen * arr_sizes[t]
                props.append(None)
            else:
                raise ValueError("unknown FBX property type %r" % t)
        children = []
        while p < end:
            child, p = read_node(p)
            if child is not None:
                children.append(child)
        return (name, props, children, offs), end

    out = []
    pos = 27
    while pos < len(data):
        node, pos = read_node(pos)
        if node is None:
            break
        out.append(node)
    return version, out


def _p70(node_children):
    """Properties70 -> {name: value} (the first value of each P record)."""
    d = {}
    for name, props, kids, _o in node_children:
        if name == "Properties70":
            for pn, pp, _k, _po in kids:
                if pn == "P" and len(pp) >= 5:
                    d[pp[0]] = pp[4] if len(pp) == 5 else tuple(pp[4:])
    return d


def fbx_raw_lights(path):
    """Every Light NodeAttribute's Properties70 (template defaults merged), in file order."""
    with open(path, "rb") as f:
        version, top = fbx_parse(f.read())
    template = {}
    for name, props, kids, _o in top:
        if name == "Definitions":
            for kn, kp, kk, _ko in kids:
                if kn == "ObjectType" and kp and kp[0] == "NodeAttribute":
                    for tn, tp, tk, _to in kk:
                        if tn == "PropertyTemplate" and tp and tp[0] == "FbxLight":
                            template = _p70(tk)
    lights = []
    for name, props, kids, _o in top:
        if name == "Objects":
            for kn, kp, kk, _ko in kids:
                if kn == "NodeAttribute" and len(kp) >= 3 and kp[2] == "Light":
                    d = dict(template)
                    d.update(_p70(kk))
                    d["_name"] = kp[1].split("\x00\x01")[0]
                    lights.append(d)
    ltype = {0: "POINT", 1: "SUN", 2: "SPOT"}
    for d in lights:
        d["_kind"] = ltype.get(d.get("LightType", 0), "?")
    return lights


def fbx_patch_intensities(src, dst, intensity_by_kind):
    """Copy a binary FBX, overwriting each Light's Intensity (an in-place 'D' double) by kind."""
    with open(src, "rb") as f:
        data = bytearray(f.read())
    version, top = fbx_parse(bytes(data))
    ltype = {0: "POINT", 1: "SUN", 2: "SPOT"}
    patched = []
    for name, props, kids, _o in top:
        if name != "Objects":
            continue
        for kn, kp, kk, _ko in kids:
            if kn == "NodeAttribute" and len(kp) >= 3 and kp[2] == "Light":
                for pn, pp, pk, _po in kk:
                    if pn != "Properties70":
                        continue
                    lt = 0
                    for qn, qp, _qk, _qo in pk:
                        if qn == "P" and qp and qp[0] == "LightType":
                            lt = qp[4]
                    for qn, qp, _qk, qo in pk:
                        if qn == "P" and qp and qp[0] == "Intensity":
                            t, off = qo[4]
                            assert t == "D", "Intensity is not a double (%s)" % t
                            kind = ltype[lt]
                            struct.pack_into("<d", data, off, intensity_by_kind[kind])
                            patched.append((kind, intensity_by_kind[kind]))
    with open(dst, "wb") as f:
        f.write(bytes(data))
    return patched


def usda_patch_intensities(src, dst, intensity_by_kind):
    """Copy a .usda, replacing each light prim's inputs:intensity by kind (SPOT = SphereLight with a cone)."""
    text = _usda_text(src)
    out = []
    pos = 0
    patched = []
    for m in re.finditer(r'def (DistantLight|SphereLight) "([^"]+)"', text):
        i = m.end()
        depth = 0
        j = i
        while j < len(text):
            c = text[j]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
            elif c == "{" and depth == 0:
                break
            j += 1
        k = j
        braces = 0
        while k < len(text):
            if text[k] == "{":
                braces += 1
            elif text[k] == "}":
                braces -= 1
                if braces == 0:
                    break
            k += 1
        body = text[j:k]
        kind = "SUN" if m.group(1) == "DistantLight" else ("SPOT" if "inputs:shaping:cone:angle" in body else "POINT")
        nb, n = re.subn(r"(float inputs:intensity = )[-\d.eE+]+", r"\g<1>%r" % intensity_by_kind[kind], body)
        assert n == 1, "no single inputs:intensity in %s" % m.group(2)
        out.append(text[pos:j])
        out.append(nb)
        pos = k
        patched.append((kind, intensity_by_kind[kind]))
    out.append(text[pos:])
    with open(dst, "w") as f:
        f.write("".join(out))
    return patched


def _usda_text(path):
    if path.lower().endswith(".usdz"):
        with zipfile.ZipFile(path) as z:
            name = next(n for n in z.namelist() if n.lower().endswith((".usda", ".usd")))
            return z.read(name).decode("utf-8", "replace")
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        return f.read()


def usd_raw_lights(path):
    """UsdLux light prims of a .usda/.usdz: kind, intensity, exposure, normalize, radius, ..."""
    text = _usda_text(path)
    lights = []
    for m in re.finditer(r'def (DistantLight|SphereLight|DiskLight|RectLight|DomeLight) "([^"]+)"', text):
        i = m.end()
        # header "( ... )" may hold a nested customData { }, so find the first "{" at paren depth 0
        depth = 0
        j = i
        while j < len(text):
            c = text[j]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
            elif c == "{" and depth == 0:
                break
            j += 1
        header = text[i:j]
        k = j
        braces = 0
        while k < len(text):
            if text[k] == "{":
                braces += 1
            elif text[k] == "}":
                braces -= 1
                if braces == 0:
                    break
            k += 1
        body = text[j:k]
        attrs = {}
        for am in re.finditer(r"^\s*(?:uniform\s+|custom\s+)?(\w+)(?:\[\])?\s+([\w:]+)\s*=\s*(.+?)\s*$", body, re.M):
            attrs[am.group(2)] = am.group(3)

        def num(key, default=None):
            v = attrs.get(key)
            try:
                return float(v)
            except (TypeError, ValueError):
                return default
        cust = re.search(r"dictionary litegen = \{(.*?)\}", header, re.S)
        cust_intensity = None
        if cust:
            im = re.search(r"(?:float|double) intensity = ([-\d.eE+]+)", cust.group(1))
            cust_intensity = float(im.group(1)) if im else None
        prim = m.group(1)
        kind = {"DistantLight": "SUN", "SphereLight": "POINT"}.get(prim, prim)
        if prim == "SphereLight" and "inputs:shaping:cone:angle" in attrs:
            kind = "SPOT"
        lights.append({
            "_name": m.group(2), "_prim": prim, "_kind": kind,
            "intensity": num("inputs:intensity"),  # None = not authored (sparse writers omit schema fallbacks)
            "exposure": num("inputs:exposure", 0.0),
            "normalize": attrs.get("inputs:normalize"),
            "radius": num("inputs:radius"), "angle": num("inputs:angle"),
            "treatAsPoint": attrs.get("treatAsPoint"),
            "cone_angle": num("inputs:shaping:cone:angle"),
            "cone_softness": num("inputs:shaping:cone:softness"),
            "customData_litegen_intensity": cust_intensity,
        })
    return lights


# ---------------------------------------------------------------------------------------------
# Blender side
# ---------------------------------------------------------------------------------------------

def clean():
    for coll in (bpy.data.objects, bpy.data.lights, bpy.data.cameras, bpy.data.meshes,
                 bpy.data.materials, bpy.data.images, bpy.data.collections):
        for x in list(coll):
            coll.remove(x)


def read_lights():
    out = []
    for o in bpy.data.objects:
        if o.type != "LIGHT":
            continue
        L = o.data
        out.append({
            "object": o.name, "data": L.name, "kind": L.type,
            "energy": float(L.energy), "exposure": float(L.exposure),
            "color": [round(float(c), 6) for c in L.color],
            "radius": float(L.shadow_soft_size),
            "normalize": bool(L.normalize),
            "spot_size": float(L.spot_size) if L.type == "SPOT" else None,
            "spot_blend": float(L.spot_blend) if L.type == "SPOT" else None,
            "sun_angle": float(L.angle) if L.type == "SUN" else None,
        })
    for L in bpy.data.lights:
        if L.users == 0 or not any(o.type == "LIGHT" and o.data == L for o in bpy.data.objects):
            out.append({
                "object": None, "data": L.name, "kind": L.type, "orphan": True,
                "energy": float(L.energy), "exposure": float(L.exposure),
                "color": [round(float(c), 6) for c in L.color],
                "radius": float(L.shadow_soft_size), "normalize": bool(L.normalize),
                "spot_size": float(L.spot_size) if L.type == "SPOT" else None,
                "spot_blend": float(L.spot_blend) if L.type == "SPOT" else None,
                "sun_angle": float(L.angle) if L.type == "SUN" else None,
            })
    out.sort(key=lambda d: (KIND_ORDER.get(d["kind"], 9), d["data"]))
    return out


def run_import(op, path, **kw):
    clean()
    err = None
    try:
        res = op(filepath=path, **kw)
    except Exception as e:  # report it, keep probing
        res = None
        err = "%s: %s" % (type(e).__name__, str(e).strip()[:300])
    return {"result": sorted(res) if res else None, "error": err, "lights": read_lights()}


LEG = "import_scene.fbx (legacy)"
LEG_STUB = "import_scene.fbx (legacy, Cycles cast_shadow stub)"


def legacy_fbx_with_cycles_stub(p, **kw):
    """Blender 5.2.0's legacy importer dies on every light at import_fbx.py L2255
    (`lamp.cycles.cast_shadow = ...`: CyclesLightSettings has no such property any more).
    Stub the property so the rest of the import runs; removed again afterwards."""
    tmp = bpy.data.lights.new("_probe_tmp", "POINT")
    cls = type(tmp.cycles)  # cycles.properties.CyclesLightSettings (not exposed in bpy.types)
    bpy.data.lights.remove(tmp)
    added = False
    if not hasattr(cls, "cast_shadow"):
        cls.cast_shadow = bpy.props.BoolProperty(default=True)
        added = True
    try:
        return bpy.ops.import_scene.fbx(filepath=p, **kw)
    finally:
        if added:
            del cls.cast_shadow


IMPORTERS = {
    "glb": [("import_scene.gltf", lambda p, **kw: bpy.ops.import_scene.gltf(filepath=p, **kw))],
    "fbx": [("wm.fbx_import", lambda p, **kw: bpy.ops.wm.fbx_import(filepath=p, **kw)),
            (LEG, lambda p, **kw: bpy.ops.import_scene.fbx(filepath=p, **kw)),
            (LEG_STUB, legacy_fbx_with_cycles_stub)],
    "usd": [("wm.usd_import", lambda p, **kw: bpy.ops.wm.usd_import(filepath=p, **kw))],
}


def import_all(kind, path, **kw):
    res = {}
    for name, fn in IMPORTERS[kind]:
        res[name] = run_import(lambda filepath, **k: fn(filepath, **k), path, **kw)
    return res


def fmt(v, w=12):
    if v is None:
        return "-".rjust(w)
    if isinstance(v, float):
        return ("%.8g" % v).rjust(w)
    return str(v).rjust(w)


def print_lights(label, lights, err=None):
    print("  [%s]%s" % (label, ("  ERROR " + err) if err else ""))
    for L in lights:
        print("    %-6s obj=%-14s data=%-14s energy=%s exposure=%s radius=%s normalize=%s spot_size=%s spot_blend=%s sun_angle=%s" % (
            L["kind"], L["object"] or "(no object)", L["data"], fmt(L["energy"]), fmt(L["exposure"], 6), fmt(L["radius"], 6),
            L["normalize"], fmt(L["spot_size"], 8), fmt(L["spot_blend"], 8), fmt(L["sun_angle"], 8)))


def print_raw(label, raws, keys):
    print("  [%s raw]" % label)
    for r in raws:
        print("    %-6s name=%-14s " % (r["_kind"], r["_name"]) + " ".join("%s=%s" % (k, r.get(k)) for k in keys))


def by_kind(lights):
    d = {}
    for L in lights:
        d.setdefault(L["kind"], []).append(L)
    return d


# ---------------------------------------------------------------------------------------------
# Steps
# ---------------------------------------------------------------------------------------------

FBX_KEYS = ("LightType", "Intensity", "Exposure", "Color", "DecayType", "InnerAngle", "OuterAngle", "ConeAngle")
USD_KEYS = ("intensity", "exposure", "normalize", "radius", "angle", "treatAsPoint", "cone_angle", "cone_softness",
            "customData_litegen_intensity")


def stage_file(out, tag, path):
    d = os.path.join(out, "inputs", tag)
    os.makedirs(d, exist_ok=True)
    dst = os.path.join(d, os.path.basename(path))
    shutil.copy2(path, dst)
    return dst


def step_inputs(out, args, R):
    print("\n=== STEP 1: environment ===")
    env = {"version_string": bpy.app.version_string, "version": list(bpy.app.version),
           "build_hash": bpy.app.build_hash.decode(), "build_date": bpy.app.build_date.decode()}
    for key, op in (("gltf_lighting_mode_default", bpy.ops.import_scene.gltf),):
        env[key] = op.get_rna_type().properties["export_import_convert_lighting_mode"].default
    env["usd_import_light_intensity_scale_default"] = bpy.ops.wm.usd_import.get_rna_type().properties["light_intensity_scale"].default
    try:
        bpy.ops.wm.collada_import.get_rna_type()
        env["wm.collada_import"] = True
    except Exception:
        env["wm.collada_import"] = False
    env["ops_wm_import_export"] = [n for n in dir(bpy.ops.wm) if "import" in n or "export" in n]
    print(json.dumps(env, indent=1))
    R["environment"] = env

    for g in args.glb or []:
        print("\n=== glTF import: %s ===" % g)
        dst = stage_file(out, "glb", g)
        r = import_all("glb", dst)
        for name, v in r.items():
            print_lights(name + " (default lighting mode " + env["gltf_lighting_mode_default"] + ")", v["lights"], v["error"])
        R.setdefault("gltf", {})[g] = r
    for f in args.fbx or []:
        print("\n=== FBX import: %s ===" % f)
        dst = stage_file(out, "fbx", f)
        raws = fbx_raw_lights(dst)
        print_raw("fbx", raws, FBX_KEYS)
        r = import_all("fbx", dst)
        for name, v in r.items():
            print_lights(name, v["lights"], v["error"])
        R.setdefault("fbx", {})[f] = {"raw": raws, "import": r}
    for u in args.usd or []:
        print("\n=== USD import: %s ===" % u)
        dst = stage_file(out, "usd", u)
        raws = usd_raw_lights(dst)
        print_raw("usd", raws, USD_KEYS)
        r = import_all("usd", dst)
        for name, v in r.items():
            print_lights(name, v["lights"], v["error"])
        R.setdefault("usd", {})[u] = {"raw": raws, "import": r}


def step_export_glb_scene(out, args, R):
    if not args.glb:
        return
    glb = stage_file(out, "glb", args.glb[0])
    print("\n=== STEP 3: export the glTF-imported scene (%s) ===" % args.glb[0])
    clean()
    bpy.ops.import_scene.gltf(filepath=glb)
    src = read_lights()
    print_lights("glTF import (the reference energies E)", src)
    exp_dir = os.path.join(out, "exports")
    os.makedirs(exp_dir, exist_ok=True)
    fbx_path = os.path.join(exp_dir, "gltf_scene_blender_export.fbx")
    usda_path = os.path.join(exp_dir, "gltf_scene_blender_export.usda")
    bpy.ops.export_scene.fbx(filepath=fbx_path)
    bpy.ops.wm.usd_export(filepath=usda_path)
    res = {"source": src}
    raws = fbx_raw_lights(fbx_path)
    print_raw("export_scene.fbx", raws, FBX_KEYS)
    r = import_all("fbx", fbx_path)
    for name, v in r.items():
        print_lights("re-import via " + name, v["lights"], v["error"])
    res["fbx"] = {"raw": raws, "import": r}
    raws = usd_raw_lights(usda_path)
    print_raw("wm.usd_export", raws, USD_KEYS)
    r = import_all("usd", usda_path)
    for name, v in r.items():
        print_lights("re-import via " + name, v["lights"], v["error"])
    res["usd"] = {"raw": raws, "import": r}
    R["export_of_gltf_scene"] = res


# name, kind, energy, exposure, extras
SYN = [
    ("POINT_e1", "POINT", 1.0, 0.0, {}),
    ("POINT_e10", "POINT", 10.0, 0.0, {}),
    ("POINT_e0p5", "POINT", 0.5, 0.0, {}),
    ("POINT_e10_x1", "POINT", 10.0, 1.0, {}),
    ("POINT_e10_r0p5", "POINT", 10.0, 0.0, {"shadow_soft_size": 0.5}),
    ("POINT_e10_unnorm", "POINT", 10.0, 0.0, {"normalize": False}),
    ("SPOT_e1", "SPOT", 1.0, 0.0, {}),
    ("SPOT_e10", "SPOT", 10.0, 0.0, {}),
    ("SPOT_e10_x1", "SPOT", 10.0, 1.0, {}),
    ("SPOT_e10_cone", "SPOT", 10.0, 0.0, {"spot_size": 0.8, "spot_blend": 0.25}),
    ("SUN_e1", "SUN", 1.0, 0.0, {}),
    ("SUN_e10", "SUN", 10.0, 0.0, {}),
    ("SUN_e0p5", "SUN", 0.5, 0.0, {}),
    ("SUN_e10_x1", "SUN", 10.0, 1.0, {}),
    ("SUN_e10_ang0p5", "SUN", 10.0, 0.0, {"angle": 0.5}),
    ("SUN_e10_unnorm", "SUN", 10.0, 0.0, {"normalize": False}),
    # energies whose UsdLux intensity equals the schema fallback (SphereLight 1.0, DistantLight 50000):
    # the exporter writes sparsely, so inputs:intensity is omitted from the file
    ("POINT_epi", "POINT", math.pi, 0.0, {}),
    ("SUN_e200000", "SUN", 200000.0, 0.0, {}),
]


def build_synthetic():
    clean()
    for i, (name, kind, energy, expo, extra) in enumerate(SYN):
        L = bpy.data.lights.new(name, kind)
        L.energy = energy
        L.exposure = expo
        for k, v in extra.items():
            setattr(L, k, v)
        o = bpy.data.objects.new(name, L)
        o.location = (i * 1.0, 0.0, 2.0)
        bpy.context.scene.collection.objects.link(o)


def step_synthetic(out, args, R):
    print("\n=== STEP 4a: Blender-authored lights -> FBX / USD export -> raw values -> re-import ===")
    exp_dir = os.path.join(out, "exports")
    os.makedirs(exp_dir, exist_ok=True)
    build_synthetic()
    src = {L["data"]: L for L in read_lights()}
    fbx_path = os.path.join(exp_dir, "synthetic_blender_export.fbx")
    usda_path = os.path.join(exp_dir, "synthetic_blender_export.usda")
    bpy.ops.export_scene.fbx(filepath=fbx_path)
    bpy.ops.wm.usd_export(filepath=usda_path)
    syn = {"source": list(src.values())}

    fraw = {r["_name"]: r for r in fbx_raw_lights(fbx_path)}
    fre = import_all("fbx", fbx_path)
    fre_by = {n: {L["data"]: L for L in v["lights"]} for n, v in fre.items()}
    print("  FBX export: Intensity vs Blender energy and re-import energy")
    print("    %-18s %-6s %9s %5s | %10s %12s | %s" % ("light", "kind", "energy", "expo", "raw Intens", "raw/(E*2^x)", "re-import energy (exposure): wm.fbx_import | legacy as-is | legacy+Cycles stub"))
    rows = []
    for (name, kind, energy, expo, extra) in SYN:
        raw = fraw.get(name, {})
        ri = raw.get("Intensity")
        ratio = ri / (src[name]["energy"] * 2 ** expo) if ri is not None else None
        a = fre_by["wm.fbx_import"].get(name)
        b = fre_by[LEG].get(name)
        c = fre_by[LEG_STUB].get(name)
        print("    %-18s %-6s %9.4g %5.1f | %10s %12s | %s (%s) | %s (%s) | %s (%s)" % (
            name, kind, energy, expo, fmt(ri, 10), fmt(ratio, 12),
            fmt(a["energy"] if a else None, 10), fmt(a["exposure"] if a else None, 3),
            fmt(b["energy"] if b else None, 10), fmt(b["exposure"] if b else None, 3),
            fmt(c["energy"] if c else None, 10), fmt(c["exposure"] if c else None, 3)))
        rows.append({"name": name, "kind": kind, "energy": energy, "exposure": expo, "raw_intensity": ri,
                     "raw_over_energy_2x": ratio,
                     "reimport_default": a, "reimport_legacy": b, "reimport_legacy_cycles_stub": c})
    syn["fbx"] = {"rows": rows, "import": fre}

    uraw = {r["_name"]: r for r in usd_raw_lights(usda_path)}
    ure = import_all("usd", usda_path)
    ure_by = {n: {L["data"]: L for L in v["lights"]} for n, v in ure.items()}
    # imported USD objects are named after the prim; match by object name too
    ure_obj = {n: {L["object"]: L for L in v["lights"]} for n, v in ure.items()}
    print("  USD export: UsdLux values vs Blender energy, and wm.usd_import re-import")
    print("    %-18s %-6s %9s %5s | %11s %9s %9s %6s %8s %6s | %12s %10s %10s" % (
        "light", "kind", "energy", "expo", "intensity", "expo", "radius", "norm", "angle", "TAP", "E/intensity", "reimp E", "reimp/E"))
    urows = []
    for (name, kind, energy, expo, extra) in SYN:
        raw = uraw.get(name, {})
        it = raw.get("intensity")  # None = omitted (sparse write of the schema fallback)
        e_over_i = src[name]["energy"] / it if it else None
        a = ure_by["wm.usd_import"].get(name) or ure_obj["wm.usd_import"].get(name)
        ratio = a["energy"] / src[name]["energy"] if a else None
        print("    %-18s %-6s %9.4g %5.1f | %11s %9s %9s %6s %8s %6s | %12s %10s %10s" % (
            name, kind, energy, expo, fmt(it, 11), fmt(raw.get("exposure"), 9), fmt(raw.get("radius"), 9),
            raw.get("normalize"), fmt(raw.get("angle"), 8), raw.get("treatAsPoint"), fmt(e_over_i, 12),
            fmt(a["energy"] if a else None, 10), fmt(ratio, 10)))
        urows.append({"name": name, "kind": kind, "energy": energy, "exposure": expo, "raw": raw,
                      "energy_over_intensity": e_over_i, "reimport": a, "reimport_over_energy": ratio})
    syn["usd"] = {"rows": urows, "import": ure}
    R["synthetic_blender_authored"] = syn

    print("\n=== STEP 4b: hand-written UsdLux files -> wm.usd_import ===")
    cases = []
    for kind, prim in (("SUN", "DistantLight"), ("POINT", "SphereLight"), ("SPOT", "SphereLight")):
        for tag, attrs in (
            ("i1", {"intensity": 1.0}),
            ("i5", {"intensity": 5.0}),
            ("i5_x1", {"intensity": 5.0, "exposure": 1.0}),
            ("i5_unnorm", {"intensity": 5.0, "normalize": False}),
            ("i5_r0p5", {"intensity": 5.0, "radius": 0.5}),
            ("i5_tap", {"intensity": 5.0, "radius": 0.5, "treatAsPoint": True}),
            ("i5_ang3", {"intensity": 5.0, "angle": 3.0}),
            ("noint", {}),
        ):
            if kind == "SUN" and ("radius" in attrs or "treatAsPoint" in attrs):
                continue
            if kind != "SUN" and "angle" in attrs:
                continue
            cases.append((kind, prim, "%s_%s" % (kind, tag), attrs))
    lines = ["#usda 1.0", "(", '    defaultPrim = "root"', '    metersPerUnit = 1', '    upAxis = "Y"', ")", "", 'def Xform "root"', "{"]
    for kind, prim, name, attrs in cases:
        api = ' (\n        prepend apiSchemas = ["ShapingAPI"]\n    )' if kind == "SPOT" else ""
        lines.append('    def %s "%s"%s' % (prim, name, api))
        lines.append("    {")
        if "intensity" in attrs:
            lines.append("        float inputs:intensity = %r" % attrs["intensity"])
        if "exposure" in attrs:
            lines.append("        float inputs:exposure = %r" % attrs["exposure"])
        if "normalize" in attrs:
            lines.append("        bool inputs:normalize = %d" % int(attrs["normalize"]))
        if "radius" in attrs:
            lines.append("        float inputs:radius = %r" % attrs["radius"])
        if "treatAsPoint" in attrs:
            lines.append("        bool treatAsPoint = 1")
        if "angle" in attrs:
            lines.append("        float inputs:angle = %r" % attrs["angle"])
        if kind == "SPOT":
            lines.append("        float inputs:shaping:cone:angle = 30")
            lines.append("        float inputs:shaping:cone:softness = 0.25")
        lines.append("    }")
    lines.append("}")
    hw = os.path.join(exp_dir, "handwritten_usdlux.usda")
    with open(hw, "w") as f:
        f.write("\n".join(lines) + "\n")
    variants = (("light_intensity_scale=1 (default)", {}), ("light_intensity_scale=2", {"light_intensity_scale": 2.0}))
    hrows = {}
    for vname, kw in variants:
        r = run_import(lambda filepath, **k: bpy.ops.wm.usd_import(filepath=filepath, **k), hw, **kw)
        by = {L["object"]: L for L in r["lights"]}
        by.update({L["data"]: L for L in r["lights"]})
        print("  [%s]%s" % (vname, ("  ERROR " + r["error"]) if r["error"] else ""))
        print("    %-14s %-6s %9s %5s %7s %6s | %-6s %10s %10s %8s %9s %6s | %10s" % (
            "prim", "kind", "intensity", "expo", "radius", "norm", "kind", "energy", "exposure", "radius", "sun_angle", "norm", "E/intens"))
        rows = []
        for kind, prim, name, attrs in cases:
            L = by.get(name)
            inten = attrs.get("intensity", 1.0)
            ratio = L["energy"] / inten if L else None
            print("    %-14s %-6s %9s %5s %7s %6s | %-6s %10s %10s %8s %9s %6s | %10s" % (
                name, kind, fmt(attrs.get("intensity"), 9), fmt(attrs.get("exposure"), 5), fmt(attrs.get("radius"), 7),
                attrs.get("normalize"), L["kind"] if L else "-", fmt(L["energy"] if L else None, 10),
                fmt(L["exposure"] if L else None, 10), fmt(L["radius"] if L else None, 8),
                fmt(L["sun_angle"] if L else None, 9), L["normalize"] if L else "-", fmt(ratio, 10)))
            rows.append({"name": name, "kind": kind, "attrs": attrs, "imported": L, "energy_over_intensity": ratio})
        hrows[vname] = rows
    R["synthetic_handwritten_usdlux"] = {"file": hw, "variants": hrows}


# ---------------------------------------------------------------------------------------------
# Derived factors
# ---------------------------------------------------------------------------------------------

def spec_E_from_glb(path):
    with open(path, "rb") as f:
        b = f.read()
    (ln,) = struct.unpack_from("<I", b, 12)
    j = json.loads(b[20:20 + ln])
    out = {}
    for l in j.get("extensions", {}).get("KHR_lights_punctual", {}).get("lights", []):
        k = {"directional": "SUN", "point": "POINT", "spot": "SPOT"}[l["type"]]
        v = l.get("intensity", 1.0)
        out[k] = {"gltf_intensity": v, "E_spec": v / 683.0 if k == "SUN" else v * 4 * math.pi / 683.0, "name": l.get("name")}
    return out


def first(lst, kind):
    for L in lst:
        if L["kind"] == kind:
            return L
    return None


def step_predict(out, args, R):
    """Write what a `blender`-units litegen would write (from the glTF reference lights), import it
    with the real importers, and compare the energies with the glTF import's. Uses the first
    --fbx / --usd as the carrier (it keeps its own cones, colours and transforms)."""
    if not (args.glb and args.predict):
        return
    spec = spec_E_from_glb(args.glb[0])
    exp_dir = os.path.join(out, "exports")
    os.makedirs(exp_dir, exist_ok=True)
    print("\n=== STEP 4c: predicted `blender`-units files, imported by the real importers ===")
    P = {}
    if args.fbx:
        dst = os.path.join(exp_dir, "predicted_blender_units.fbx")
        want = {k: 100.0 * v["E_spec"] for k, v in spec.items()}  # Intensity = 100 * E
        patched = fbx_patch_intensities(args.fbx[0], dst, want)
        print("  FBX carrier %s -> Intensity %s" % (args.fbx[0], patched))
        r = import_all("fbx", dst)
        P["fbx"] = {"patched": patched, "import": r}
        for name, v in r.items():
            print_lights(name, v["lights"], v["error"])
            for k, w in spec.items():
                L = first(v["lights"], k)
                if L:
                    print("      %-6s energy/E_spec = %s" % (k, fmt(L["energy"] / w["E_spec"], 12)))
    if args.usd:
        dst = os.path.join(exp_dir, "predicted_blender_units.usda")
        want = {k: (v["E_spec"] / 4.0 if k == "SUN" else v["E_spec"] / math.pi) for k, v in spec.items()}
        patched = usda_patch_intensities(args.usd[0] if args.usd[0].lower().endswith(".usda") else
                                         next(u for u in args.usd if u.lower().endswith(".usda")), dst, want)
        print("  USDA carrier -> inputs:intensity %s" % patched)
        r = import_all("usd", dst)
        P["usd"] = {"patched": patched, "import": r}
        for name, v in r.items():
            print_lights(name, v["lights"], v["error"])
            for k, w in spec.items():
                L = first(v["lights"], k)
                if L:
                    print("      %-6s energy/E_spec = %s" % (k, fmt(L["energy"] / w["E_spec"], 12)))
    R["predicted_blender_units"] = P


def step_factors(args, R):
    print("\n=== STEP 5: derived factors (staged-style scenes: one light per kind) ===")
    F = {}
    spec = spec_E_from_glb(args.glb[0]) if args.glb else {}
    F["spec_E"] = spec

    def vs_spec(kind, energy):
        return energy / spec[kind]["E_spec"] if kind in spec and spec[kind]["E_spec"] else None

    if args.glb:
        g = R["gltf"][args.glb[0]]["import_scene.gltf"]["lights"]
        print("  glTF import energy vs spec E (lux/683 ; cd*4pi/683):")
        for k in ("SUN", "POINT", "SPOT"):
            L = first(g, k)
            if L and k in spec:
                print("    %-6s gltf=%-8s energy=%-14s spec E=%-14s ratio=%s" % (k, spec[k]["gltf_intensity"], fmt(L["energy"], 14), fmt(spec[k]["E_spec"], 14), fmt(vs_spec(k, L["energy"]), 12)))
                F.setdefault("gltf_energy_over_specE", {})[k] = vs_spec(k, L["energy"])
    for path, v in R.get("fbx", {}).items():
        print("  FBX file %s" % path)
        for imp_name, imp in v["import"].items():
            for r in v["raw"]:
                L = first(imp["lights"], r["_kind"])
                if L and r.get("Intensity") is not None and L["energy"]:
                    f = r["Intensity"] / L["energy"]
                    vs = vs_spec(r["_kind"], L["energy"])
                    print("    %-52s %-6s raw Intensity=%-12s energy=%-14s Intensity/energy=%-10s energy/E_spec=%s" % (imp_name, r["_kind"], fmt(r["Intensity"], 12), fmt(L["energy"], 14), fmt(f, 10), fmt(vs, 12)))
                    F.setdefault("fbx_import_divisor", {}).setdefault(path, {}).setdefault(imp_name, {})[r["_kind"]] = f
    for path, v in R.get("usd", {}).items():
        print("  USD file %s" % path)
        for imp_name, imp in v["import"].items():
            for r in v["raw"]:
                L = first(imp["lights"], r["_kind"])
                if L and r.get("intensity"):
                    f = L["energy"] / r["intensity"]
                    vs = vs_spec(r["_kind"], L["energy"])
                    print("    %-14s %-6s raw intensity=%-12s energy=%-14s energy/intensity=%-10s energy/E_spec=%s%s" % (imp_name, r["_kind"], fmt(r["intensity"], 12), fmt(L["energy"], 14), fmt(f, 10), fmt(vs, 12),
                          "   (customData intensity %s)" % r["customData_litegen_intensity"] if r.get("customData_litegen_intensity") is not None else ""))
                    F.setdefault("usd_import_factor", {}).setdefault(path, {})[r["_kind"]] = f
    e = R.get("export_of_gltf_scene")
    if e:
        print("  Blender export of the glTF-imported scene (E = the glTF import's energy)")
        src = e["source"]
        for r in e["fbx"]["raw"]:
            S = first(src, r["_kind"])
            A = first(e["fbx"]["import"]["wm.fbx_import"]["lights"], r["_kind"])
            B = first(e["fbx"]["import"][LEG_STUB]["lights"], r["_kind"])
            print("    FBX %-6s E=%-14s Intensity=%-14s Intensity/E=%-10s re-import default=%s (x%s) legacy+stub=%s (x%s)" % (
                r["_kind"], fmt(S["energy"], 14), fmt(r["Intensity"], 14), fmt(r["Intensity"] / S["energy"], 10),
                fmt(A["energy"], 14), fmt(A["energy"] / S["energy"], 10), fmt(B["energy"], 14), fmt(B["energy"] / S["energy"], 10)))
            F.setdefault("fbx_export_Intensity_over_E", {})[r["_kind"]] = r["Intensity"] / S["energy"]
            F.setdefault("fbx_roundtrip_ratio", {})[r["_kind"]] = A["energy"] / S["energy"]
        for r in e["usd"]["raw"]:
            if r["_kind"] not in KIND_ORDER:
                continue
            S = first(src, r["_kind"])
            A = first(e["usd"]["import"]["wm.usd_import"]["lights"], r["_kind"])
            print("    USD %-6s E=%-14s intensity=%-14s E/intensity=%-10s re-import=%s (x%s of E)" % (
                r["_kind"], fmt(S["energy"], 14), fmt(r["intensity"], 14), fmt(S["energy"] / r["intensity"], 10),
                fmt(A["energy"], 14), fmt(A["energy"] / S["energy"], 10)))
            F.setdefault("usd_export_E_over_intensity", {})[r["_kind"]] = S["energy"] / r["intensity"]
            F.setdefault("usd_roundtrip_ratio", {})[r["_kind"]] = A["energy"] / S["energy"]
    R["factors"] = F


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--glb", action="append")
    ap.add_argument("--fbx", action="append")
    ap.add_argument("--usd", action="append")
    ap.add_argument("--synthetic", action="store_true")
    ap.add_argument("--predict", action="store_true",
                    help="patch the first --fbx/--usd with `blender`-units values from the --glb and import them")
    args = ap.parse_args(argv)
    os.makedirs(args.out, exist_ok=True)
    R = {}
    step_inputs(args.out, args, R)
    step_export_glb_scene(args.out, args, R)
    if args.synthetic:
        step_synthetic(args.out, args, R)
    step_predict(args.out, args, R)
    step_factors(args, R)
    with open(os.path.join(args.out, "readings.json"), "w") as f:
        json.dump(R, f, indent=1, default=str)
    print("\nWROTE", os.path.join(args.out, "readings.json"))


main()
