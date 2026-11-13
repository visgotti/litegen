#!/usr/bin/env bash
# khronos-schemas.sh <dir>: fetch the nine Khronos material-layer schema files
# that scripts/mesh-verify/layer-extensions.test.mjs checks its defaults table
# against (KhronosGroup/glTF at the commit the table was written from), and
# check each against the sha-256 its README lists. Then KHRONOS_SCHEMAS=<dir>
# runs that test instead of skipping it.
set -euo pipefail
dir=${1:?usage: khronos-schemas.sh <dir>}
commit=9d23a5a5b1d10f22d1554e847f2a1a3f24222392
mkdir -p "$dir"
cd "$dir"
for l in anisotropy clearcoat dispersion ior iridescence sheen specular transmission volume; do
  curl -fsSL --retry 3 -o "material.KHR_materials_$l.schema.json" \
    "https://raw.githubusercontent.com/KhronosGroup/glTF/$commit/extensions/2.0/Khronos/KHR_materials_$l/schema/material.KHR_materials_$l.schema.json"
done
shasum -a 256 -c - <<'EOF'
bedd1b5c202bae64430f99358f89dd43da9ee1d2e4d5af6e32a0653ba997ff0f  material.KHR_materials_anisotropy.schema.json
56201df5bc73bebc9185f693ac9b69ad0108fdb4143b6274c3929b97c7cd2911  material.KHR_materials_clearcoat.schema.json
90b96906ca94f520658df714dddc9002c59df536786cff0ef06f239aec0f5cb1  material.KHR_materials_dispersion.schema.json
cd9649bf71d15eb0e3f2842329720ce74eccfdc2d1b383776c000ae58f660e57  material.KHR_materials_ior.schema.json
76be44f7c6fdc3cab3d10c4072228138003c0338cd35e089d8d55e6aa879f6a3  material.KHR_materials_iridescence.schema.json
fbb0f4a3a3d3e9b7b0dac4495dd4a37f77573fe1211bc9c5b6300f02193899ce  material.KHR_materials_sheen.schema.json
543465f7f2aa185b2f15c242b88df67759c4b488438282a3650002794bb70228  material.KHR_materials_specular.schema.json
ba7f35963c83c3def22235cc6958d94fa605d663ec49a65865ebc1ec61fbbea7  material.KHR_materials_transmission.schema.json
581681f6c15ab1983190c4db7e6d7b10697dea4bbf93773face7e21f2f52ff27  material.KHR_materials_volume.schema.json
EOF
