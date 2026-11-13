# Shared 3D preview: implementation plan

Status: tasks 1–6 done on 2026-11-06 by litegen-d0, with a final review and its fix pass. Not committed. Ledger: `.superpowers/sdd/2026-11-06-shared-3d-preview/progress.md`.

Spec: `docs/superpowers/specs/2026-11-06-shared-3d-preview-design.md`.
Executed inline by litegen-d0 (superpowers:executing-plans), with test-driven
development. Ledger: `.superpowers/sdd/2026-11-06-shared-3d-preview/progress.md`.

## Global constraints

- No commits, staging or stashing. No worktrees. No cargo: nothing here is Rust.
- No edits to `litegen-core/**`, `packages/3d-converter/**` or `sdks/**`.
- The dashboard's 3D preview files are claimed: litegen-b2 and litegen-59 were
  told on 2026-11-06, and nobody else is in them.
- A move is checked by the suites that already cover the moved code. New
  behaviour gets a failing test first.
- Notes go in the ledger, never only in /private/tmp.

## Task 1: the package and its helpers

- Create `packages/3d-viewer/{package.json, tsconfig.json, vitest.config.ts,
  .gitignore, README.md, src/index.ts}`.
  - Dev dependencies pinned to the tool's installed versions: react 19.3.1,
    @types/react 19.3.0, three 0.183.2, @types/three 0.183.1, lucide-react
    1.69.0, typescript 6.0.3, vitest 4.1.12.
- `src/format.ts`:
  - `formatBytes` and `formatDimensions`, the same in both apps today;
  - `drawnNote` and `copyToClipboard`.
- Tests: `src/format.test.ts` holds the union of the dashboard's and the
  tool's cases for these four. Watch it fail before `format.ts` exists.

## Task 2: the engine moves

- `src/viewer/`: `MeshViewer.tsx`, `loaders.ts`, `framing.ts`, `webgl.ts` and
  `viewer.css`.
- `src/types.ts`: `ViewerFormat`, `VIEWER_FORMATS`, `ViewerFileRole`,
  `StoredFrame` and `UpAxis`. The engine stops importing
  `@litegen/3d-converter`.
- The tool depends on `@litegen/3d-viewer` (`file:`), sets `resolve.dedupe`,
  imports the engine from it and deletes its copies.
- Test first, in the tool: `src/viewer-types.test.ts` asserts each package
  type equals the converter's (`Format`, `FileRole`, `StoredFrame`,
  `UpAxis`) and that `VIEWER_FORMATS` equals `formats()`. It is RED until the
  package exports them.
- The loader unit tests move with the loaders.

## Task 3: remote models, failures and the context budget

- Test first: `src/viewer/fetch-model.test.ts` covers `fetchModel(url, {
  fetch, signal, onProgress })`:
  - progress events are monotonic and end at 1;
  - an HTTP 404 or 500 gives `{ reason: 'load', status }`;
  - a network error gives `{ reason: 'load', status: null }`;
  - an abort gives no result.
- `RemoteMeshViewer`:
  - shows the poster until the model is drawn;
  - shows a `role=progressbar` with the percentage;
  - its fallback has a download link;
  - Retry is offered for a `load` failure only. A loader's throw counts as
    `parse`.
- `MeshViewer`:
  - `mountWhenVisible` creates the renderer only while within 200 px of the
    viewport, and on leaving releases it with `forceContextLoss`;
  - `webglcontextlost` gives the fallback with Retry;
  - `preserveDrawingBuffer` becomes a prop. The tool sets it, because its
    videos read the canvas.
- Story proof: `ManyViewers` puts 24 viewers in a scrolling column, and the
  stories project asserts at most 16 live contexts while every viewer reaches
  `loaded` once scrolled to.

## Task 4: one inspector

- `src/inspector/ModelInspector.tsx` and `inspector.css`:
  - the toolbar, holding title, auto-rotate, animation (when there are clips),
    reset, exposure, background, fullscreen and an `actions` slot;
  - the stage, holding `MeshViewer` or `RemoteMeshViewer`;
  - the stats: Format, Size, Triangles and Dimensions;
  - the drawn note;
  - `children` below.
- Parts:
  - `FactList`;
  - `ClipTable`, with an optional Loop column;
  - `FileTable`, with columns supplied by the caller;
  - `Thumbnails`;
  - `NoteList` for losses, warnings and manifest notes;
  - `InspectorError`;
  - `CopyUrlButton`.
- The tool moves to them:
  - `Inspector` is replaced by `ModelInspector`;
  - `ModelFacts`, `FileList` and `LossList` render the shared parts;
  - its texture files show as thumbnails.
- Test IDs follow spec D4, so the tool's e2e test IDs stay as they are.

## Task 5: the dashboard adopts it

- `ModelPreview.tsx` becomes an adapter. It maps `Model3dAsset[]` and
  `scene` onto `ModelInspector` and `RemoteMeshViewer`:
  - `sceneFacts` and `clipRows` stay in the dashboard, as rows;
  - `scene.notes` go to `NoteList`;
  - `scene.triangle_count` feeds the drawn note;
  - `scene.bounds` feeds Dimensions.
- `ModelViewer.tsx` (the compact tiles) is `RemoteMeshViewer` with
  `mountWhenVisible`. `ResultTile3D` is unchanged apart from that.
- Both load lazily (`React.lazy`), so three.js stays out of the main chunk.
- Removed:
  - `@google/model-viewer`;
  - from `model3d-assets.ts`: `canPreviewMesh`, `retrySrc`,
    `loadEventMatches`, `classifyViewerError`, `matchResourceStatus` and
    `describeViewerError`, with their tests;
  - `formatBytes`, `formatDimensions` and `copyToClipboard`, which come from
    the package now.
- `three` becomes a direct dependency.
- Stories: ModelPreview and ModelViewer on the dashboard fixtures, plus
  non-glTF meshes from the mesh test data.
- Checks: `npm test`, `tsc -b`, `npm run lint`, `npm run build` (three.js
  absent from the main chunk) and `npm run build-storybook`.

## Task 6: proof in both apps

- Dashboard e2e (`model3d.spec.ts`, `model3d-playground.spec.ts`,
  `model3d-packages.spec.ts`, `compare-playground.spec.ts`):
  - the `<model-viewer>` checks become `data-state=loaded` with
    `data-coverage > 0`;
  - `-stat-polycount` becomes `-stat-triangles`;
  - new: mock/mesh-3d with `output_formats: ['obj']` draws an OBJ.
  - Run against the existing release binary (no cargo).
- Tool: vitest, the stories project, and full e2e with recording.
- Both Storybooks list the package's stories.

## Review focus

1. Two React copies: a hook crash on the first render in either app.
2. The context budget: many expanded Generations rows.
3. The private file route: a `blob:` URL must still load, with progress
   reading 100%.
4. Cross-origin mesh URLs: the download link must not navigate the
   dashboard away (`target=_blank`).
5. StrictMode double effects: one renderer per viewer, none leaked.
