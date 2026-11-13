# @litegen/3d-viewer

litegen's 3D preview. The dashboard and the 3D converter tool
(`apps/3d-converter`) both show models through it, so they show them the same
way. It is private: it isn't published.

- **`MeshViewer`** draws a model in any of the eleven formats litegen's
  converter writes (glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz, usda,
  usdc). Each format
  goes through three.js's own loader for it, so what you see is what an
  independent reader makes of the file.
- **`RemoteMeshViewer`** downloads a model from a URL, with progress, then
  draws it.
  - A failed download offers Retry.
  - A file its loader can't read doesn't, because a second download brings
    the same bytes.
- **`ModelInspector`** is the viewer with its toolbar:
  - auto-rotate, animation, reset camera, exposure, background, fullscreen,
    and the caller's actions;
  - its stats: format, size, triangles, dimensions;
  - a note when the format's loader drew fewer triangles than litegen reads;
  - the caller's details below.
- **The parts:** `FactList`, `ClipTable`, `DataTable`, `NoteList`,
  `Thumbnails`, `InspectorError`, `CopyUrlButton` and `Chip`.
- **`@litegen/3d-viewer/helpers`:** the pure part, with no React and no
  three.js. It has `formatBytes`, `formatDimensions`, `drawnNote`,
  `copyToClipboard` and the format vocabulary.

Each app maps its own data onto these. The dashboard maps the API's assets
and scene manifest (`dashboard/src/components/ModelPreviewBody.tsx`). The tool
maps the converter package's `ModelInfo` and files
(`apps/3d-converter/src/components/ConverterPage.tsx`).

Design: `docs/superpowers/specs/2026-11-06-shared-3d-preview-design.md`.

## The viewer's state, for tests and tools

The viewer's root element (`[data-viewer]`) carries:

| Attribute | What |
|---|---|
| `data-state` | `idle` (off screen, with `mountWhenVisible`), `loading`, `loaded` or `error` |
| `data-reason` | On an error: `load` (Retry), `parse`, `webgl`, `context` (Retry), `import` (the loader's code didn't download: reload the page) or `format` |
| `data-format`, `data-src` | The container, and a remote model's URL as given |
| `data-coverage`, `data-silhouette` | Once loaded: how much of a 64×64 rest-pose silhouette the model covers, and that mask |
| `data-triangles`, `data-clip-coverage` | What the loader drew, and the coverage halfway through the first clip |

## Using it from an app

It is linked TypeScript source. There's no build step.

1. Depend on it: `"@litegen/3d-viewer": "file:../packages/3d-viewer"`, plus
   `three` 0.183.2, `react`, `react-dom` and `lucide-react`. These four are
   its peer dependencies.
2. In `vite.config.ts`:
   - set `resolve.dedupe: ['react', 'react-dom', 'three', 'lucide-react']`.
     The package has its own copies for its own tests, and two Reacts break
     hooks.
   - add the package's folder to `server.fs.allow`.
3. To keep three.js out of the first chunk, import the components with a
   dynamic `import()`, and the helpers from `@litegen/3d-viewer/helpers`.
4. On a page with many viewers, pass `mountWhenVisible`. A browser keeps only
   so many WebGL contexts (Chrome keeps 16), and the viewer gives its context
   back when it scrolls away.
5. A client knows no format's axis. Pass `storedFrame` (and `up`) from
   whoever does: the converter package's `formats()`, or the API.

## Tests

- `npm test`: the pure parts, in Node (vitest).
- `npm run typecheck`.
- Stories (`src/**/*.stories.tsx`): plain CSF, so the package needs no
  Storybook. Both apps' Storybooks show them, served with `fixtures/` at
  `/viewer-fixtures`. They run in a real browser in `apps/3d-converter`'s
  stories project (`e2e/stories.spec.ts`), which checks each by its tags.
- The apps' own suites cover the adapters:
  - the tool's e2e (every conversion, drawn and compared);
  - the dashboard's `e2e/model3d*.spec.ts`, against a litegen binary;
  - the dashboard's `npm run test:stories`.
