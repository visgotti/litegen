"""The PyPI package's side of parity.mjs: reads the jobs (JSON on stdin) and
prints, per job, the sha256 and size of the package's output."""

import hashlib
import json
import sys

import litegen_3d_converter as l3d

jobs = json.load(sys.stdin)
out = []
for job in jobs:
    with open(job["path"], "rb") as f:
        data = f.read()
    try:
        body = l3d.convert(data, from_format=job["from"], to_format=job["to"], package=job["package"]).files[0].data
        out.append({"sha256": hashlib.sha256(body).hexdigest(), "size": len(body)})
    except l3d.ConvertError as e:
        out.append({"sha256": None, "error": f"{e.code}: {e}"})
json.dump(out, sys.stdout)
