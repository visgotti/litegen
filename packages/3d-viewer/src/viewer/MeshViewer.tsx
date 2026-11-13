// Shows a model in any of the ten formats litegen's converter writes,
// through three.js's own loader for that format (./loaders.ts). The one
// viewer behind both the dashboard and the 3D converter tool.
//
// Its props, its handle and its loading and failure behaviour began as the
// dashboard's <model-viewer> wrapper's (dashboard/src/components/
// ModelViewer.tsx before this package). That element draws glTF only; this
// one draws with three.js directly, which is also what <model-viewer> is
// built on.
//
// The root element carries the viewer's state for tests and tools:
// data-state (idle | loading | loaded | error), data-reason on an error
// (load | parse | webgl | context | import | format), data-format, data-src (a remote
// model's URL, as given), and once loaded
// data-coverage (how much of a 64x64 rest-pose silhouette the model covers),
// data-silhouette (that mask, base64, one bit per pixel, row-major from the
// top, most significant bit first), data-triangles (what the loader drew)
// and data-clip-coverage (the coverage halfway through the first clip).
import { useEffect, useImperativeHandle, useMemo, useRef, useState, type CSSProperties, type Ref } from 'react';
import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { isViewerFormat, type StoredFrame, type UpAxis, type ViewerFile } from '../types';
import { FileServer, LoaderUnavailableError, loadModel, needsZUpTurn } from './loaders';
import { FOV, frame } from './framing';
import { hasWebGL2, WEBGL_MESSAGE } from './webgl';
import './viewer.css';

export type ViewerBackground = 'dark' | 'light' | 'checker';

/** Why the viewer shows no model:
 *  - `load`: the file never arrived (Retry can help);
 *  - `parse`: it arrived and its loader could not read it;
 *  - `webgl`: this browser has no WebGL 2, or refused the viewer a context;
 *  - `context`: the browser took the viewer's WebGL context back (Retry);
 *  - `import`: the loader's own code could not be downloaded (reload);
 *  - `format`: the viewer has no loader for the format. */
export type ViewerFailure = 'load' | 'parse' | 'webgl' | 'context' | 'import' | 'format';

export interface ViewerLoadInfo {
  /** The model's size along each axis, in its own units; null if it has none. */
  dimensions: { x: number; y: number; z: number } | null;
  /** Animation clips the loader found. */
  clips: number;
  /** Triangles the loader drew: every placed mesh, instances included. */
  triangles: number;
}

/** Camera reset is a one-shot command, so a handle, not a prop to toggle. */
export interface MeshViewerHandle {
  resetCamera(): void;
}

export interface MeshViewerProps {
  /** The model file and the files it references; the one whose role is
   *  `model` (or the first) is loaded. Null while the bytes are on their way. */
  files: ViewerFile[] | null;
  /** The container, lowercase. One the viewer has no loader for shows the
   *  download fallback (`data-reason="format"`). */
  format: string;
  /** The frame the file stores: the converter package's `formats()` says,
   *  or the API. Unknown is taken as +Y up, which turns nothing. */
  storedFrame?: StoredFrame;
  /** For a format whose frame is the caller's (STL, OBJ, PLY): its up axis. */
  up?: UpAxis;
  autoRotate?: boolean;
  /** Play the first animation clip. */
  animate?: boolean;
  exposure?: number;
  background?: ViewerBackground;
  /** An image to show until the model is drawn. */
  poster?: string | null;
  /** While `files` is null: the share downloaded, 0 to 1, or null when
   *  unknown. */
  progress?: number | null;
  /** A download that failed, shown in place of the model, with Retry when
   *  `onRetry` is given. */
  failure?: string | null;
  onRetry?: () => void;
  /** Where the fallback's download link points; else the model file itself. */
  downloadUrl?: string;
  /** Draw only while within 200px of the viewport, and give the WebGL
   *  context back when scrolled away: for pages with many viewers, since a
   *  browser keeps only so many contexts (Chrome: 16) and drops the oldest. */
  mountWhenVisible?: boolean;
  testId?: string;
  style?: CSSProperties;
  onLoad?: (info: ViewerLoadInfo) => void;
  onError?: (message: string) => void;
  ref?: Ref<MeshViewerHandle>;
}

type Status =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'loaded'; coverage: number; silhouette: string; triangles: number; clipCoverage: number | undefined }
  | { status: 'error'; reason: ViewerFailure; message: string };

/** Keyed by the load it belongs to, so a new model (or a new attempt) reads
 *  as loading from its very first render, never as the last one's result. */
type State = Status & { key: unknown };

/** The parts of a running viewer that effects other than the load need. */
interface Running {
  renderer: THREE.WebGLRenderer;
  camera: THREE.PerspectiveCamera;
  controls: OrbitControls;
  root: THREE.Object3D | null;
  mixer: THREE.AnimationMixer | null;
  clips: THREE.AnimationClip[];
}

const SILHOUETTE = 64;

const CONTEXT_MESSAGE = 'The browser took back the WebGL context this viewer was drawing with.';
const REFUSED_MESSAGE = "The browser refused this viewer a WebGL context, so the model can't be previewed — download it instead.";

/** The model at rest, white on black, from the framing's viewpoint: its
 *  coverage and its one-bit mask. Both viewers of a conversion frame by the
 *  same rule, so the input's mask and the output's compare. */
function silhouette(renderer: THREE.WebGLRenderer, scene: THREE.Scene, root: THREE.Object3D, framing?: ReturnType<typeof frame>): { coverage: number; mask: string } {
  const f = framing ?? frame(root, 1);
  const camera = new THREE.PerspectiveCamera(FOV, 1, f.near, f.far);
  camera.position.copy(f.position);
  camera.lookAt(f.target);
  camera.updateMatrixWorld();
  const target = new THREE.WebGLRenderTarget(SILHOUETTE, SILHOUETTE);
  const white = new THREE.MeshBasicMaterial({ color: 0xffffff, side: THREE.DoubleSide });
  const saved = { background: scene.background, environment: scene.environment, override: scene.overrideMaterial, tone: renderer.toneMapping };
  scene.background = new THREE.Color(0x000000);
  scene.environment = null;
  scene.overrideMaterial = white;
  renderer.toneMapping = THREE.NoToneMapping;
  const pixels = new Uint8Array(SILHOUETTE * SILHOUETTE * 4);
  try {
    renderer.setRenderTarget(target);
    renderer.render(scene, camera);
    renderer.readRenderTargetPixels(target, 0, 0, SILHOUETTE, SILHOUETTE, pixels);
  } finally {
    renderer.setRenderTarget(null);
    scene.background = saved.background;
    scene.environment = saved.environment;
    scene.overrideMaterial = saved.override;
    renderer.toneMapping = saved.tone;
    target.dispose();
    white.dispose();
  }
  const bits = new Uint8Array((SILHOUETTE * SILHOUETTE) / 8);
  let covered = 0;
  for (let row = 0; row < SILHOUETTE; row++) {
    // GL rows run bottom-up; the mask runs top-down.
    const glRow = SILHOUETTE - 1 - row;
    for (let col = 0; col < SILHOUETTE; col++) {
      if (pixels[(glRow * SILHOUETTE + col) * 4]! > 127) {
        const i = row * SILHOUETTE + col;
        bits[i >> 3]! |= 0x80 >> (i & 7);
        covered++;
      }
    }
  }
  return { coverage: covered / (SILHOUETTE * SILHOUETTE), mask: btoa(String.fromCharCode(...bits)) };
}

function disposeObject(root: THREE.Object3D) {
  root.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (!mesh.isMesh) return;
    mesh.geometry?.dispose();
    for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) {
      if (!m) continue;
      for (const value of Object.values(m)) if (value instanceof THREE.Texture) value.dispose();
      m.dispose();
    }
  });
}

/** How much of the rest pose's frame the model covers halfway through
 *  `clip`, seen from the rest pose's viewpoint: a clip that throws the model
 *  out of frame (a misread rotation) shows as a small number. Every node's
 *  pose is put back afterwards. */
function coverageMidClip(renderer: THREE.WebGLRenderer, scene: THREE.Scene, root: THREE.Object3D, animated: THREE.Object3D, clip: THREE.AnimationClip): number {
  const rest = frame(root, 1);
  const saved: Array<[THREE.Object3D, THREE.Vector3, THREE.Quaternion, THREE.Vector3]> = [];
  root.traverse((o) => saved.push([o, o.position.clone(), o.quaternion.clone(), o.scale.clone()]));
  const mixer = new THREE.AnimationMixer(animated);
  try {
    mixer.clipAction(clip).play();
    mixer.setTime(clip.duration / 2);
    return silhouette(renderer, scene, root, rest).coverage;
  } finally {
    mixer.stopAllAction();
    mixer.uncacheRoot(animated);
    for (const [o, p, q, sc] of saved) {
      o.position.copy(p);
      o.quaternion.copy(q);
      o.scale.copy(sc);
    }
    root.updateMatrixWorld(true);
  }
}

/** Triangles drawn under `root`: each mesh's, times its instances. */
function trianglesIn(root: THREE.Object3D): number {
  let n = 0;
  root.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (!mesh.isMesh) return;
    const g = mesh.geometry;
    const count = g.index ? g.index.count : (g.getAttribute('position')?.count ?? 0);
    const instances = (o as THREE.InstancedMesh).isInstancedMesh ? (o as THREE.InstancedMesh).count : 1;
    n += Math.floor(count / 3) * instances;
  });
  return n;
}

const baseName = (path: string) => path.slice(path.lastIndexOf('/') + 1);

/** The name a URL's file goes by: the last part of its path, when it has
 *  one with an extension (a `blob:` URL does not). */
function fileNameOf(url: string, format: string): string {
  let path = url;
  try {
    path = new URL(url, 'https://x.invalid/').pathname;
  } catch {
    // Not a URL: the string itself is the path.
  }
  let name = baseName(path);
  try {
    name = decodeURIComponent(name);
  } catch {
    // A malformed escape: keep it as it is.
  }
  return /\.[a-z0-9]+$/i.test(name) ? name : `model.${format}`;
}

/** The one presentation for every way the viewer can't show a model: the
 *  file is still downloadable, so it degrades to a link rather than an empty
 *  box. Retry is offered only where a second attempt can help. */
function Fallback({ reason, message, href, fileName, testId, onRetry }: {
  reason: ViewerFailure;
  message: string;
  href: string | null;
  fileName: string;
  testId?: string;
  onRetry?: () => void;
}) {
  return (
    <div role="alert" className="mv-fallback" data-reason={reason} data-testid={testId ? `${testId}-fallback` : undefined}>
      <span>{message}</span>
      <span className="mv-fallback-actions">
        {href && (
          // target=_blank: a remote URL is usually cross-origin, where
          // browsers ignore `download`, and must not navigate the page away.
          <a href={href} download={fileName} target="_blank" rel="noopener noreferrer" data-testid={testId ? `${testId}-fallback-link` : undefined}>
            Download {fileName}
          </a>
        )}
        {onRetry && (
          <button type="button" className="btn btn-secondary mv-retry" data-testid={testId ? `${testId}-retry` : undefined} onClick={onRetry}>
            Retry
          </button>
        )}
      </span>
    </div>
  );
}

/** The main file as a blob: URL, for the fallback's download link. */
function useBlobHref(files: ViewerFile[] | null, wanted: boolean): string | null {
  const main = files ? (files.find((f) => f.role === 'model') ?? files[0]) : undefined;
  const [href, setHref] = useState<{ file: ViewerFile; url: string } | null>(null);
  useEffect(() => {
    if (!main || !wanted) return;
    const url = URL.createObjectURL(new Blob([main.data as Uint8Array<ArrayBuffer>]));
    setHref({ file: main, url });
    return () => URL.revokeObjectURL(url);
  }, [main, wanted]);
  return wanted && href && href.file === main ? href.url : null;
}

export default function MeshViewer({
  files,
  format,
  storedFrame,
  up,
  autoRotate = true,
  animate = true,
  exposure = 1,
  background = 'dark',
  poster,
  progress,
  failure,
  onRetry,
  downloadUrl,
  mountWhenVisible = false,
  testId,
  style,
  onLoad,
  onError,
  ref,
}: MeshViewerProps) {
  const host = useRef<HTMLDivElement | null>(null);
  const running = useRef<Running | null>(null);
  const [stored, setState] = useState<State>({ status: 'loading', key: null });
  // A Retry after a lost context starts the same model over.
  const [attempt, setAttempt] = useState(0);
  const [inView, setInView] = useState(!mountWhenVisible);
  // Callbacks are read at call time, so a new closure does not reload.
  const callbacks = useRef({ onLoad, onError });
  callbacks.current = { onLoad, onError };
  const settings = useRef({ autoRotate, animate, exposure });
  settings.current = { autoRotate, animate, exposure };

  const drawable = isViewerFormat(format) ? format : null;
  const visible = !mountWhenVisible || inView;
  // What stops the viewer before it starts: known from the props alone. No
  // WebGL 2 stops it before any download too (RemoteMeshViewer asks first).
  const blocked: { reason: ViewerFailure; message: string } | null = failure
    ? { reason: 'load', message: failure }
    : !drawable
      ? { reason: 'format', message: `3D preview isn't available for ${format ? `${format.toUpperCase()} files` : 'this format'} — download the model.` }
      : !hasWebGL2()
        ? { reason: 'webgl', message: WEBGL_MESSAGE }
        : null;
  const active = !blocked && files !== null && visible;
  // Every change of what is shown comes with a new files array (a new read,
  // conversion or download) or URL, and every return to the viewport or
  // Retry is a new load too: this object is that load's identity.
  const loadKey = useMemo(
    () => ({ files, format, storedFrame, up, downloadUrl, attempt, active }),
    [files, format, storedFrame, up, downloadUrl, attempt, active],
  );
  const state: Status = blocked
    ? { status: 'error', ...blocked }
    : files === null
      ? { status: 'loading' }
      : !visible
        ? { status: 'idle' }
        : stored.key === loadKey
          ? stored
          : { status: 'loading' };

  // Said once per load it stops, so a caller keying its state by model (an
  // inspector) hears it for each new model, even with the same message.
  const blockedMessage = blocked?.message;
  useEffect(() => {
    if (blockedMessage) callbacks.current.onError?.(blockedMessage);
  }, [blockedMessage, loadKey]);

  useEffect(() => {
    if (!mountWhenVisible) return;
    const el = host.current;
    if (!el || typeof IntersectionObserver === 'undefined') {
      setInView(true);
      return;
    }
    // Entries come in time order: the last says where the viewer is now.
    const observer = new IntersectionObserver((entries) => {
      const now = entries[entries.length - 1];
      if (now) setInView(now.isIntersecting);
    }, { rootMargin: '200px' });
    observer.observe(el);
    return () => observer.disconnect();
  }, [mountWhenVisible]);

  useEffect(() => {
    const el = host.current;
    if (!el || !active || !files || !drawable) return;
    let cancelled = false;
    let contextLost = false;
    // This load reports to the callbacks of its own render: one that ends
    // after a newer model was given never speaks for the newer one.
    const report = callbacks.current;
    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true });
    } catch {
      // The probe found WebGL 2, but the browser refused this context (after
      // GPU resets, say): fall back rather than let the throw take the page.
      setState({ status: 'error', reason: 'webgl', message: REFUSED_MESSAGE, key: loadKey });
      report.onError?.(REFUSED_MESSAGE);
      return;
    }
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.setClearColor(0x000000, 0);
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.NeutralToneMapping;
    renderer.toneMappingExposure = settings.current.exposure;
    renderer.domElement.className = 'mv-canvas';
    el.prepend(renderer.domElement);
    const onContextLost = (e: Event) => {
      // Our own forceContextLoss() on the way out fires this too.
      if (cancelled) return;
      e.preventDefault();
      // A load still in flight must not report a blank canvas as drawn.
      contextLost = true;
      renderer.setAnimationLoop(null);
      setState({ status: 'error', reason: 'context', message: CONTEXT_MESSAGE, key: loadKey });
      report.onError?.(CONTEXT_MESSAGE);
    };
    renderer.domElement.addEventListener('webglcontextlost', onContextLost);

    const scene = new THREE.Scene();
    const pmrem = new THREE.PMREMGenerator(renderer);
    const environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
    scene.environment = environment;
    // The loaders that build Phong and Lambert materials (OBJ, FBX, Collada)
    // are lit by lights, not by the environment.
    scene.add(new THREE.HemisphereLight(0xffffff, 0x3a3f47, 1.1));
    const key = new THREE.DirectionalLight(0xffffff, 1.4);
    key.position.set(1, 2, 1.5);
    scene.add(key);

    const camera = new THREE.PerspectiveCamera(FOV, 1, 0.01, 1000);
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.autoRotate = settings.current.autoRotate;
    controls.autoRotateSpeed = 1.5;
    const run: Running = { renderer, camera, controls, root: null, mixer: null, clips: [] };
    running.current = run;

    const resize = () => {
      const width = Math.max(el.clientWidth, 1);
      const height = Math.max(el.clientHeight, 1);
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(el);
    resize();

    // THREE.Timer, which replaced the deprecated Clock (r183 warns on Clock).
    const timer = new THREE.Timer();
    renderer.setAnimationLoop((time) => {
      timer.update(time);
      const dt = timer.getDelta();
      run.mixer?.update(dt);
      controls.update(dt);
      renderer.render(scene, camera);
    });

    const server = new FileServer(files);
    (async () => {
      const loaded = await loadModel(files, drawable, server);
      if (cancelled || contextLost) return disposeObject(loaded.root);
      // Its textures, which most loaders fetch after handing the model back.
      await server.settled(15_000, () => cancelled || contextLost);
      if (cancelled || contextLost) return disposeObject(loaded.root);
      const root = new THREE.Group();
      if (needsZUpTurn(storedFrame, up)) root.rotation.x = -Math.PI / 2;
      root.add(loaded.root);
      scene.add(root);
      run.root = root;
      run.clips = loaded.clips;
      const f = frame(root, camera.aspect);
      camera.position.copy(f.position);
      camera.near = f.near;
      camera.far = f.far;
      camera.updateProjectionMatrix();
      controls.target.copy(f.target);
      controls.update();
      controls.saveState();
      // The silhouette is the model at rest: before any clip plays.
      const shot = silhouette(renderer, scene, root);
      const clipCoverage = loaded.clips.length > 0 ? coverageMidClip(renderer, scene, root, loaded.root, loaded.clips[0]!) : undefined;
      if (loaded.clips.length > 0) {
        run.mixer = new THREE.AnimationMixer(loaded.root);
        if (settings.current.animate) run.mixer.clipAction(loaded.clips[0]!).play();
      }
      const triangles = trianglesIn(root);
      // The context can go while the silhouette is read: then nothing drew.
      if (cancelled || contextLost) return;
      setState({ status: 'loaded', coverage: shot.coverage, silhouette: shot.mask, triangles, clipCoverage, key: loadKey });
      const size = f.size;
      report.onLoad?.({ dimensions: size.lengthSq() > 0 ? { x: size.x, y: size.y, z: size.z } : null, clips: loaded.clips.length, triangles });
    })().catch((e: unknown) => {
      if (cancelled || contextLost) return;
      const [reason, message]: [ViewerFailure, string] = e instanceof LoaderUnavailableError
        ? ['import', `${e.message} Reload the page to try again.`]
        : ['parse', `This ${drawable.toUpperCase()} file could not be shown: ${e instanceof Error ? e.message : String(e)}`];
      setState({ status: 'error', reason, message, key: loadKey });
      report.onError?.(message);
    });

    return () => {
      cancelled = true;
      renderer.domElement.removeEventListener('webglcontextlost', onContextLost);
      observer.disconnect();
      renderer.setAnimationLoop(null);
      controls.dispose();
      if (run.root) disposeObject(run.root);
      environment.dispose();
      pmrem.dispose();
      renderer.dispose();
      // dispose() keeps the WebGL context until garbage collection, and a
      // browser keeps only so many (Chrome: 16), losing the oldest live one
      // first. Give it back now.
      renderer.forceContextLoss();
      renderer.domElement.remove();
      server.dispose();
      running.current = null;
    };
    // loadKey changes exactly when one of the others does.
  }, [loadKey, active, files, drawable, storedFrame, up]);

  useEffect(() => {
    const run = running.current;
    if (run) run.controls.autoRotate = autoRotate;
  }, [autoRotate]);

  useEffect(() => {
    const run = running.current;
    if (run) run.renderer.toneMappingExposure = exposure;
  }, [exposure]);

  useEffect(() => {
    const run = running.current;
    if (!run?.mixer || run.clips.length === 0) return;
    if (animate) run.mixer.clipAction(run.clips[0]!).play();
    // Stopping every action restores the model's rest pose.
    else run.mixer.stopAllAction();
  }, [animate, state.status]);

  useImperativeHandle(ref, () => ({
    resetCamera() {
      running.current?.controls.reset();
    },
  }), []);

  const failed = state.status === 'error';
  const main = files ? (files.find((f) => f.role === 'model') ?? files[0]) : undefined;
  const blobHref = useBlobHref(files, failed && !downloadUrl);
  const href = downloadUrl ?? blobHref;
  const fileName = main ? baseName(main.name) : fileNameOf(downloadUrl ?? '', format);
  const retry = failed
    ? state.reason === 'context'
      ? () => setAttempt((n) => n + 1)
      : state.reason === 'load'
        ? onRetry
        : undefined
    : undefined;
  const pct = progress == null ? null : Math.round(Math.min(Math.max(progress, 0), 1) * 100);

  return (
    <div
      ref={host}
      className={`mv mv--${background}`}
      style={style}
      data-viewer=""
      data-testid={testId}
      data-state={state.status}
      data-reason={state.status === 'error' ? state.reason : undefined}
      data-format={format}
      data-src={downloadUrl}
      data-coverage={state.status === 'loaded' ? state.coverage.toFixed(4) : undefined}
      data-silhouette={state.status === 'loaded' ? state.silhouette : undefined}
      data-triangles={state.status === 'loaded' ? state.triangles : undefined}
      data-clip-coverage={state.status === 'loaded' ? state.clipCoverage?.toFixed(4) : undefined}
    >
      {poster && (state.status === 'idle' || state.status === 'loading') && (
        <img className="mv-poster" src={poster} alt="" data-testid={testId ? `${testId}-poster` : undefined} />
      )}
      {state.status === 'loading' && (
        <div
          className="mv-progress"
          role="progressbar"
          aria-label="Loading model"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={files === null && pct != null ? pct : undefined}
          data-testid={testId ? `${testId}-progress` : undefined}
        >
          {files === null && pct != null && <div className="mv-progress-fill" style={{ width: `${Math.max(pct, 2)}%` }} />}
        </div>
      )}
      {state.status === 'error' && (
        <Fallback reason={state.reason} message={state.message} href={href} fileName={fileName} testId={testId} onRetry={retry} />
      )}
    </div>
  );
}
