#!/usr/bin/env python3
"""Skinned mesh NOT at identity (Lcl Translation x=5): with/without BindPose; Transform per file convention.
Usage: variants3.py SCRIPTS_DIR OUT_DIR"""
import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("minifbx", os.path.join(sys.argv[1], "minifbx.py"))
out = sys.argv[2]; sys.argv = [sys.argv[0], out]
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
def T(x, y, z): return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1]
for with_pose in (True, False):
    r = m.full(7400)
    objs = [n for n in r if n.name == "Objects"][0]
    for o in objs.children:
        if o.name == "Model" and o.props[0][1] == 2001:
            p70 = [c for c in o.children if c.name == "Properties70"][0]
            for p in p70.children:
                if p.props[0][1] == b"Lcl Translation":
                    p.props[4:7] = [m.D(5.0), m.D(0.0), m.D(0.0)]
        if o.name == "Deformer" and o.props[2][1] == b"Cluster":
            for c in o.children:
                if c.name == "Transform":
                    c.props = [m.ad(T(5, 0, 0) if o.props[0][1] == 4002 else T(5, -1, 0))]
        if o.name == "Pose":
            for pn in o.children:
                if pn.name == "PoseNode" and pn.children[0].props[0][1] == 2001:
                    pn.children[1].props = [m.ad(T(5, 0, 0))]
    if not with_pose:
        objs.children = [o for o in objs.children if o.name != "Pose"]
    m.write(os.path.join(out, "skin_offset_%s.fbx" % ("pose" if with_pose else "nopose")), r, version=7400)
print("ok")
