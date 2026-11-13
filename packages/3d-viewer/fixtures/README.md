# Story fixtures

Small real models for the package's stories. Both Storybooks (the dashboard's
and the 3D converter tool's) serve this folder at `/viewer-fixtures`.

| File | From |
|---|---|
| `cube.glb`, `corrupt.glb`, `preview.png` | `dashboard/.storybook/fixtures` (written by its `make_fixtures.py`) |
| `cube.obj` | `apps/3d-converter/public/samples/textured-obj/model.obj` (its `mtllib` is not served: a remote model is shown as one file) |
| `cube.mtl` | `apps/3d-converter/public/samples/textured-obj/model.mtl` (its textures are left out on purpose: a material library whose images are missing) |
| `textured.3mf` | `litegen-core/src/mesh/testdata/3mf/assimp_textured.3mf` (written by assimp; stored +Z up) |
