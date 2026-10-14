#!/usr/bin/env python3
"""Extra probe variants built on minifbx.py. Usage: variants2.py SCRIPTS_DIR OUT_DIR"""
import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("minifbx", os.path.join(sys.argv[1], "minifbx.py"))
sys.argv = [sys.argv[0], sys.argv[2]]
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
OUT = sys.argv[1]
ver = 7400
# 1) header + objects + connections only (no GlobalSettings)
objects, conns = m.scene()
m.write(os.path.join(OUT, "v_hdr_obj.fbx"), [m.header(ver), objects, conns], version=ver)
# 2) material without children, with and without NULL record
def strip_material(sentinel):
    r = m.full(ver)
    objs = [n for n in r if n.name == "Objects"][0]
    for o in objs.children:
        if o.name == "Material":
            o.children = []
    if sentinel:
        m.ALWAYS_SENTINEL.add("Material")
    else:
        m.ALWAYS_SENTINEL.discard("Material")
    m.write(os.path.join(OUT, "v_mat_nochildren_%s.fbx" % ("null" if sentinel else "nonull")), r, version=ver)
strip_material(True); strip_material(False)
m.ALWAYS_SENTINEL.discard("Material")
# 3) FBXHeaderExtension with only FBXVersion
r = m.full(ver)
r[0] = m.N("FBXHeaderExtension").add(m.N("FBXHeaderVersion", m.I(1003)), m.N("FBXVersion", m.I(ver)))
m.write(os.path.join(OUT, "v_minhdr.fbx"), r, version=ver)
# 4) no Takes, no footer, Connections last
r = [n for n in m.full(ver) if n.name != "Takes"]
m.write(os.path.join(OUT, "v_notakes_nofooter.fbx"), r, version=ver, footer=False)
# 5) no BindPose
r = m.full(ver)
objs = [n for n in r if n.name == "Objects"][0]
objs.children = [o for o in objs.children if o.name != "Pose"]
m.write(os.path.join(OUT, "v_nobindpose.fbx"), r, version=ver)
print("ok")
