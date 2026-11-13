# One 3D preview for the converter tool and the dashboard

Status: decided by litegen-d0 on 2026-11-06, under the user's standing rule
"take my own recommendation on decisions and say which". Not committed.

## 1. Why

The user asked on 2026-11-06: "we're making sure all the preview stuff in the
tool and also the playground in litegen dashboard has parity and re-uses what
it can, right?"

It wasn't. The tool (`apps/3d-converter`) shows models with a hand copy of the
dashboard's inspector. The copy's chrome was ported from `ModelPreview.tsx`
and `ModelPreview.css`, and its helpers from `model3d-assets.ts`, with
`<model-viewer>` replaced by a three.js viewer. Nothing imports across the two
apps and nothing compares them, so they drifted apart:

| | Dashboard | Tool |
|---|---|---|
| Engine | `<model-viewer>`, glTF only. Any other format shows "3D preview isn't available". | three.js's own loader for each of the 10 formats |
| Animation | never plays (no `autoplay`) | plays, with a toggle |
| Only here | copy URL; Retry after a network failure; progress %; poster; texture thumbnails; scene facts and clips table with a Loop column; vendor original link | the drawn-triangles note; the r183 text-USD rotation fix; Collada vertex colours; the WebGL context release |

## 2. Goals

- G1. One package, `packages/3d-viewer` (`@litegen/3d-viewer`), that both apps
  import. It holds the viewer engine, the inspector chrome, the display parts
  (facts, clips, file tables, thumbnails, notes, the error panel), the
  formatting helpers and the CSS.
- G2. Every feature in section 1 reaches both apps, except where it means
  nothing in one of them. Copy URL needs a URL, and the tool's models are bytes
  in memory.
- G3. The dashboard previews all ten formats and plays animations.
- G4. Each app keeps only an adapter that maps its own data to the shared
  props. The dashboard maps the API's assets and scene manifest. The tool maps
  the converter package's `ModelInfo` and files.
- G5. Proof:
  - the tool's suites pass unchanged in what they assert: vitest, the stories
    project and e2e (the matrix covers 140 conversions);
  - the dashboard's 3D e2e passes on the shared package and gains a
    non-glTF case.

## 3. Non-goals

- No feature beyond the union in section 1. No AR, no measuring tools.
- The dashboard still doesn't open zip packages (`model3d-assets.ts`: "Nothing
  here opens a zip"). An OBJ result therefore shows its geometry without its
  `.mtl`, which only the package zip carries.
- `@litegen/3d-viewer` isn't published.

## 4. Constraints

- **Thin clients.** Section 6 of `2026-11-05-3d-converter-packages-design.md`
  sets the approved reading of the rule: a client "knows no format's axis"
  and asks the source of truth for it.
  - The tool asks the converter package (`formats()`: `storedFrame`, `label`).
  - The dashboard must ask the API, and today's API doesn't say. Until it
    does, the dashboard passes no frame and the viewer turns nothing. That is
    right for every container the gateway delivers except 3MF, which the
    writer turns to +Z up (`threemf.rs:153`). Derived OBJ, STL and PLY are
    written in the scene's +Y frame: `model3d_deliver.rs:791` passes no up
    axis.
  - Follow-up for the user (litegen-b2 can't take it now): `GET
    /v1/models3d/convert/formats` gains `details: Model3dFormatDetail[]`, one
    per `formats` entry, where `Model3dFormatDetail { format, label,
    stored_frame }` and `Model3dStoredFrame = "y_up" | "z_up" | "declared" |
    "caller"`. The table's natural home is `litegen-core/src/mesh`, which the
    mesh sessions own.
  - The viewer package's own knowledge is about three.js only:
    - which loaders apply a declared axis themselves (Collada);
    - r183's quaternion order for text USD;
    - Collada's vertex colours being left off.
- **No `<model-viewer>`.** The `@google/model-viewer` dependency is removed.
- **WebGL context budget.**
  - `<model-viewer>` draws every element on a page with one shared context.
    Ours costs one context per viewer, and Chrome keeps 16 live, dropping the
    oldest.
  - The dashboard can show many viewers at once: Generations rows expanded
    together, and compare tiles.
  - So a viewer given `mountWhenVisible` creates its renderer only while it is
    near the viewport, and gives the context back (`forceContextLoss`) when it
    leaves or unmounts.
  - A context the browser takes anyway shows the fallback with Retry.
- **Lazy three.js in the dashboard.** As with `<model-viewer>` before, a
  session that never shows a 3D result never downloads the 3D bundle.
- **One React and one three.js at runtime.** The package declares them as
  peer dependencies, and both apps set `resolve.dedupe`.

## 5. Decisions

- D1 **Package.** `packages/3d-viewer` is TS/TSX and CSS source with no build
  step, consumed as `file:` dependencies.
  - Peer dependencies: `react`, `react-dom`, `three` 0.183.2, `lucide-react`.
  - Its dev dependencies are pinned to the tool's installed versions, for its
    own `tsc` and vitest.
  - Each component imports its own CSS.
- D2 **Engine.** The tool's three.js `MeshViewer` is the only engine. Its
  props stop naming the converter package's types. The package defines
  `ViewerFormat`, `ViewerFileRole`, `StoredFrame` and `UpAxis` itself, and the
  tool's type test pins each one equal to the converter's.
- D3 **Remote models.** `RemoteMeshViewer` fetches one URL into memory with
  progress, then shows it with `MeshViewer`.
  - A network failure or HTTP status of 400 or above is a `load` failure, and
    Retry is offered.
  - A file whose loader throws is a `parse` failure. Retry would fetch the same
    bytes, so it isn't offered.
  - The status is the fetch's own, so the `PerformanceObserver` and `#retry-N`
    workarounds for `<model-viewer>` go, with their tests.
  - The dashboard's private file route downloads through its own client
    (`fetchStoredFile`), as `useFileSrc` does for images. The client
    answers an API error as that HTTP status.
  - A loader's own code that can't be downloaded is an `import` failure.
    That's usually a page left open across a deploy. The viewer says to
    reload, with no Retry: browsers keep a failed module import.
  - The dashboard wraps both lazy imports in an error boundary
    (`ViewerBoundary`). A chunk gone after a deploy then leaves the mesh's
    download link, and the rest of the dashboard stays up.
- D4 **Test IDs.** Each control and part has one test ID. Where both apps had
  one, it's the dashboard's name:
  - `-autorotate`, `-animate`, `-reset`, `-exposure`, `-bg-<value>`,
    `-fullscreen`, `-copy-url`;
  - `-viewer`, plus `-viewer-progress`, `-viewer-fallback`,
    `-viewer-fallback-link` and `-viewer-retry`;
  - `-stats` with `-stat-format`, `-stat-size`, `-stat-triangles` and
    `-stat-dimensions`;
  - `-facts`, `-clips`, `-files` or `-assets`, `-thumbs`, `-notes`,
    `-drawn-note`.
- D5 **Stats.**
  - "Triangles" replaces "Polycount". The API documents `polycount` as the
    triangle count.
  - Format reads as the uppercase container code in both apps. The tool adds
    the format's label as a tooltip.
  - Dimensions come from the producer when it measured them (the converter's
    info, the manifest's bounds) and from the viewer otherwise.
- D6 **Stage height.** 420 px in the inspector, 240 px in a compare tile.

## 6. Testing

- Package vitest: the moved helpers, plus the new fetch/progress/failure
  classification as a pure function over an injected `fetch`.
- Stories:
  - the package's stories for the shared parts show in both Storybooks;
  - the tool's stories project still loads every `viewer` story, with the
    count updated;
  - a "many viewers" story proves the context budget: live canvases stay
    within the cap while scrolling, and each viewer loads when it comes into
    view.
- Tool e2e: unchanged. It is the proof that the move broke nothing.
- Dashboard e2e, against the release binary:
  - "model-viewer reached `loaded`" becomes "the shared viewer is
    `data-state=loaded` with nonzero coverage";
  - `-stat-polycount` becomes `-stat-triangles`;
  - a new case shows an OBJ-only result drawn.
