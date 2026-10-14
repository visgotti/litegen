#!/usr/bin/env python3
"""UnitScaleFactor / axis probes. Usage: variants4.py SCRIPTS_DIR OUT_DIR"""
import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("minifbx", os.path.join(sys.argv[1], "minifbx.py"))
out = sys.argv[2]; sys.argv = [sys.argv[0], out]
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
def setgs(r, **kv):
    gs = [n for n in r if n.name == "GlobalSettings"][0]
    p70 = [c for c in gs.children if c.name == "Properties70"][0]
    for p in p70.children:
        k = p.props[0][1].decode()
        if k in kv:
            v = kv[k]
            p.props[4] = m.D(float(v)) if k.endswith("ScaleFactor") else m.I(int(v))
for name, kv in [("usf1", dict(UnitScaleFactor=1, OriginalUnitScaleFactor=1)),
                 ("usf100", dict(UnitScaleFactor=100, OriginalUnitScaleFactor=100)),
                 ("zup_usf1", dict(UnitScaleFactor=1, OriginalUnitScaleFactor=1, UpAxis=2, FrontAxis=1, FrontAxisSign=-1, OriginalUpAxis=2))]:
    r = m.full(7400); setgs(r, **kv)
    m.write(os.path.join(out, "units_%s.fbx" % name), r, version=7400)
print("ok")
