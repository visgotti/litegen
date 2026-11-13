# 3D converter (apps/3d-converter)

A browser tool that converts 3D models between glb, gltf, obj, stl, ply, 3mf,
dae, fbx, usdz, usda and usdc, and shows the model before and after. It is planned
for tools.litegen.ai/3d-converter.

Everything happens in the browser. The conversion is
[`@litegen/3d-converter`](../../packages/3d-converter/js/README.md), litegen's
own converter compiled to WebAssembly, running in a Web Worker. Nothing is
uploaded.

This is not the proxy dashboard, and the Docker image does not build it. It
is a static site of its own.

## What it does

1. **Your model**: drop files or a zip, or pick one of ten samples (one per
   source format). Several files are read as one model: a `.gltf` with its
   `.bin` and images, an OBJ with its `.mtl` and textures.
2. **Convert to**: pick a target. The options that apply to the source and
   the target show up: units and up axis for STL, OBJ and PLY, the FBX colour
   space, the animation frame rate, and the packaging (one file, a zip, or
   separate files).
3. **Output**: the converted model is shown the same way as the input,
   through three.js's own loader for its format. Next to it are the
   downloads, what the target couldn't hold, and the files written.

The page is built from the LiteGen dashboard's design system, ported
rather than shared, so the dashboard stays untouched:

- `src/index.css`: the dashboard's brand tokens and its shared classes
  (`.btn`, `.input`, `.badge`, `.alert`, `.page-title`, the playground's
  panels, form groups and tabs);
- `src/App.css`: its application shell (the branded sidebar beside the
  content column, a top bar on phones) and the converter's panels.

Each viewer is the dashboard's own 3D inspector: both apps import it from
`packages/3d-viewer` (`@litegen/3d-viewer`), so the two show models the same
way. It has auto-rotate, an animation toggle, reset camera, exposure,
background and fullscreen. Below it are what the converter measured
(triangles, bounds in metres, materials, skin and clips), the textures, and
each file's role.

The page holds no format knowledge of its own. Which file is the model, which
options apply, what a format keeps, what was lost: every one of those comes
from the package, as litegen's "clients stay thin" rule asks.

## Run it

```sh
(cd ../../packages/3d-converter/js && npm install && node scripts/build.mjs)   # the package, once
npm install
npm run dev            # http://localhost:5181
npm run build          # dist/: static files, served from any path
npm run preview        # http://localhost:5182, the built dist/
```

`dist/` is plain static files with relative URLs (`base: './'`), so any
static host can serve it, under any path. The `.wasm` must be served as
`application/wasm` (most hosts already do).

## Develop

```sh
npm test                    # vitest: helpers, file intake, the worker client
npm run storybook           # the viewer, the inspector and every panel, on real sample files
npm run samples             # rewrites public/samples from litegen's test data
```

The Vite settings that matter, in `vite.config.ts`:

- `worker.format: 'es'`, because the package uses a top-level `await`;
- `optimizeDeps.exclude` for the package, so its `.wasm` stays beside it;
- `server.fs.allow`, so the dev server can serve the package and the
  samples from outside this folder.

## End-to-end proof

```sh
npm run build && npm run build-storybook
npx playwright install chromium
PW_PROJECT=stories npx playwright test --project stories   # every story renders
PW_PROJECT=e2e npx playwright test --project e2e           # the converter, recorded
npm run contact-sheet                                      # e2e-artifacts/contact-sheet.png
npm run videos                                             # e2e-artifacts/videos/*.mp4 (needs ffmpeg)
```

The e2e project drives the built app in Chromium, recorded on video:

- **The matrix.** Each of the ten samples converts to every target, once as
  one file and once as separate files for the formats with sidecars. Each
  output must render. For a static model, its silhouette must match the
  input's, 0.85 overlap or better, in the frame its format stores. A
  screenshot of each result goes to `e2e-artifacts/shots`.
- **The rest.** Multi-file input, a zip in and a zip out, the options, and a
  corrupt file.

`npm run videos` turns the recordings into one MP4 per test, plus
`all-conversions.mp4` joining them in order.

## What three.js's loaders don't show

The viewers draw each file with three.js r183's own loader for its format, so
a loader's gaps show as they are. The tool works around three, and names the
rest:

- **Worked around in the shared viewer (`packages/3d-viewer/src/viewer/loaders.ts`):**
  - the USD loader reads a text layer's skeleton rotations in the wrong
    order, so an animated USDA or USDZ would fly apart; they're reordered;
  - the Collada loader leaves vertex colours off; they're turned on.
- **Not worked around:**
  - the USD loader draws per-face materials (GeomSubset bindings) in one
    colour, and some quads in `displayColor`;
  - the USDZ loader skips a referenced prim: Apple's sample draws 3 of its 4
    meshes. The inspector says when a loader drew fewer triangles than the
    converter read.

In every case the converted file holds what the converter wrote; the files
list and the losses say so.
