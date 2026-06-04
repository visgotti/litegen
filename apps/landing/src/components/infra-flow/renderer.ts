/**
 * Renderer engine for the infrastructure-flow visual — backend-agnostic.
 *
 * Responsibilities (the React component owns the DOM + interaction state):
 *   - pick a draw backend: WebGPU first, then a pixel-identical WebGL2 fallback;
 *     return null when neither is available so the component shows the SVG,
 *   - each frame, read the live on-screen rect of every node relative to the
 *     stage, ease per-node hover focus + the cursor, pack the std140 uniform,
 *     and hand it to the backend to draw,
 *   - keep it light on phones / low-end machines: cap the render resolution and
 *     frame-rate by device tier, and stop the loop entirely while the diagram is
 *     scrolled out of view or the tab is hidden,
 *   - tear everything down on destroy().
 *
 * The caller passes live, mutable `pointer` and `focus` objects; the engine
 * reads them every frame without any per-frame allocation or React involvement.
 */
import { createWebGPUBackend } from './backend-webgpu';
import { createWebGL2Backend } from './backend-webgl2';
import {
  FLAGS_BASE,
  GEOM_BASE,
  MAX_NODES,
  META_BASE,
  UNIFORM_FLOATS,
  type FlowBackend,
  type FlowHandle,
  type PointerState,
} from './types';

export type { FlowHandle, PointerState } from './types';

export interface RendererOpts {
  canvas: HTMLCanvasElement;
  stage: HTMLElement;
  nodes: FlowHandle[];
  /** Live pointer state, mutated by the component (not reassigned). */
  pointer: PointerState;
  /** Live per-node target focus (0..1), keyed by node id. */
  focus: Map<string, number>;
  reducedMotion: boolean;
  /**
   * Aborts a half-finished setup. Because WebGPU device acquisition is async, a
   * fast unmount (or StrictMode's mount→cleanup→mount) could otherwise create a
   * device and reconfigure the shared canvas *after* teardown.
   */
  signal?: AbortSignal;
  /**
   * Called when the GPU device/context is lost unexpectedly (driver reset,
   * browser reclaiming it) — NOT as part of our own teardown. Lets the component
   * fall back to the static SVG instead of freezing on a dead canvas.
   */
  onLost?: () => void;
  /**
   * Debug override (via the `?renderer=` URL flag) to compare backends:
   *   'webgpu' — only try WebGPU (→ SVG if unavailable),
   *   'webgl2' — skip WebGPU, force the WebGL2 fallback,
   *   'svg'    — skip both, force the static SVG.
   * Default (undefined) is the normal WebGPU→WebGL2→SVG chain.
   */
  forceBackend?: 'webgpu' | 'webgl2' | 'svg';
}

export interface FlowRenderer {
  /** Which backend actually initialized — handy for the debug badge. */
  readonly kind: 'webgpu' | 'webgl2';
  destroy(): void;
  /**
   * Request a single repaint. A no-op while the animation loop is running; under
   * reduced motion — where there is no rAF loop — this is how hover focus changes
   * get reflected. Coalesced to one frame.
   */
  requestRedraw(): void;
}

const EASE = 0.12;

/** Per-device render budget. Phones / low-end machines render fewer pixels less often. */
interface Tier {
  lowPower: boolean;
  dprCap: number;
  /** Minimum ms between drawn frames (0 = uncapped / native rAF). */
  minFrameMs: number;
}

function detectTier(): Tier {
  if (typeof navigator === 'undefined') return { lowPower: false, dprCap: 2, minFrameMs: 0 };
  const mm = typeof window !== 'undefined' && typeof window.matchMedia === 'function';
  const coarse = mm && window.matchMedia('(pointer: coarse)').matches;
  const small = mm && window.matchMedia('(max-width: 920px)').matches;
  const cores = navigator.hardwareConcurrency || 8;
  const mem = (navigator as Navigator & { deviceMemory?: number }).deviceMemory ?? 8;
  const low = coarse || small || cores <= 4 || mem <= 4;
  // Half-resolution-ish + ~40fps on low-end is far smoother and barely softer;
  // full 2× DPR + native rAF on capable machines.
  return { lowPower: low, dprCap: low ? 1.5 : 2, minFrameMs: low ? 1000 / 40 : 0 };
}

function roleNum(role: FlowHandle['role']): number {
  if (role === 'app') return 0;
  if (role === 'gateway') return 1;
  return 2;
}

export async function createFlowRenderer(opts: RendererOpts): Promise<FlowRenderer | null> {
  if (opts.signal?.aborted) return null;

  const { canvas, stage, nodes, pointer, focus, reducedMotion } = opts;
  const tier = detectTier();

  let destroyed = false;
  let lost = false;
  const handleLost = () => {
    lost = true;
    if (raf) {
      cancelAnimationFrame(raf);
      raf = 0;
    }
    opts.onLost?.();
  };

  // Backend selection: WebGPU, then the WebGL2 fallback, then give up (→ SVG).
  // `forceBackend` (from the ?renderer= flag) lets us pin one for comparison.
  const force = opts.forceBackend;
  if (force === 'svg') return null;

  let backend: FlowBackend | null = null;
  if (force !== 'webgl2') {
    backend = await createWebGPUBackend({
      canvas,
      lowPower: tier.lowPower,
      onLost: handleLost,
      signal: opts.signal,
    });
    if (opts.signal?.aborted) {
      backend?.destroy();
      return null;
    }
  }
  if (!backend && force !== 'webgpu') {
    backend = createWebGL2Backend({ canvas, lowPower: tier.lowPower, onLost: handleLost });
  }
  if (!backend || opts.signal?.aborted) {
    backend?.destroy();
    return null;
  }
  const backendKind = backend.kind;

  const nodeList = nodes.slice(0, MAX_NODES);
  const data = new Float32Array(UNIFORM_FLOATS);

  // Eased state lives here so it survives between frames.
  const easedFocus = new Map<string, number>(nodeList.map((n) => [n.id, 0]));
  const easedPointer = { x: pointer.x, y: pointer.y };

  let raf = 0;
  let lastDraw = -1e9;
  let inView = true;
  let pageHidden = typeof document !== 'undefined' && document.hidden;

  const t0 = typeof performance !== 'undefined' ? performance.now() : 0;

  function ensureSize(rect: DOMRect): number {
    const dpr = Math.min(typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1, tier.dprCap);
    const w = Math.max(1, Math.round(rect.width * dpr));
    const h = Math.max(1, Math.round(rect.height * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
      backend!.resize(w, h);
    }
    return dpr;
  }

  function drawFrame(now: number) {
    if (destroyed || lost) return;

    const rect = stage.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return; // not laid out yet

    const dpr = ensureSize(rect);
    const t = (now - t0) / 1000;

    if (reducedMotion) {
      easedPointer.x = pointer.x;
      easedPointer.y = pointer.y;
    } else {
      easedPointer.x += (pointer.x - easedPointer.x) * EASE;
      easedPointer.y += (pointer.y - easedPointer.y) * EASE;
    }

    data[0] = canvas.width;
    data[1] = canvas.height;
    data[2] = dpr;
    data[3] = nodeList.length;
    data[4] = reducedMotion ? 0 : t;
    data[5] = reducedMotion ? 1 : 0;
    data[6] = 0;
    data[7] = 0;
    data[8] = easedPointer.x;
    data[9] = easedPointer.y;
    data[10] = pointer.inside ? 1 : 0;
    data[11] = 0;

    for (let i = 0; i < nodeList.length; i++) {
      const node = nodeList[i];
      const b = node.el.getBoundingClientRect();
      const cx = (b.left + b.width / 2 - rect.left) / rect.width;
      const cy = (b.top + b.height / 2 - rect.top) / rect.height;
      const radius = Math.max(b.width, b.height) / 2 / rect.width;

      const target = focus.get(node.id) ?? 0;
      let eased = easedFocus.get(node.id) ?? 0;
      eased += (target - eased) * EASE;
      easedFocus.set(node.id, eased);

      const g = GEOM_BASE + i * 4;
      data[g] = cx;
      data[g + 1] = cy;
      data[g + 2] = radius;
      data[g + 3] = reducedMotion ? target : eased;

      // meta.z/.w carry the node's half-extents (fraction of stage w/h) so the
      // shader draws a rounded-box halo that hugs rectangular cards.
      const m = META_BASE + i * 4;
      data[m] = roleNum(node.role);
      data[m + 1] = node.kind === 'video' ? 1 : 0;
      data[m + 2] = b.width / 2 / rect.width;
      data[m + 3] = b.height / 2 / rect.height;

      data[FLAGS_BASE + i * 4] = node.inputsMask ?? 0;
      data[FLAGS_BASE + i * 4 + 1] = node.outputsMask ?? 0;
    }

    backend!.draw(data);
  }

  // ---- animation loop (normal motion) -------------------------------------
  function tick(now: number) {
    raf = 0;
    if (destroyed || lost || !inView || pageHidden) return;
    // Frame-rate cap (low-end tier): skip the heavy GPU draw, keep the rAF cheap.
    if (tier.minFrameMs > 0 && now - lastDraw < tier.minFrameMs) {
      raf = requestAnimationFrame(tick);
      return;
    }
    lastDraw = now;
    drawFrame(now);
    raf = requestAnimationFrame(tick);
  }
  function resume() {
    if (raf || destroyed || lost || reducedMotion || !inView || pageHidden) return;
    raf = requestAnimationFrame(tick);
  }
  function pauseLoop() {
    if (raf) {
      cancelAnimationFrame(raf);
      raf = 0;
    }
  }

  // Pause work while the diagram is scrolled off-screen — the biggest win on
  // mobile/battery, since the loop otherwise runs forever regardless of visibility.
  let io: IntersectionObserver | undefined;
  let resizeObserver: ResizeObserver | undefined;
  let onVisibility: (() => void) | undefined;

  if (reducedMotion) {
    // No loop: one static frame, redrawn only on resize (or hover via requestRedraw).
    drawFrame(t0);
    if (typeof ResizeObserver !== 'undefined') {
      resizeObserver = new ResizeObserver(() => drawFrame(t0));
      resizeObserver.observe(stage);
    }
  } else {
    if (typeof IntersectionObserver !== 'undefined') {
      io = new IntersectionObserver(
        (entries) => {
          inView = entries.some((e) => e.isIntersecting);
          if (inView) resume();
          else pauseLoop();
        },
        { rootMargin: '200px' },
      );
      io.observe(stage);
    }
    if (typeof document !== 'undefined') {
      onVisibility = () => {
        pageHidden = document.hidden;
        if (pageHidden) pauseLoop();
        else resume();
      };
      document.addEventListener('visibilitychange', onVisibility);
    }
    resume();
  }

  let redrawQueued = false;

  return {
    kind: backendKind,
    destroy() {
      destroyed = true;
      pauseLoop();
      io?.disconnect();
      resizeObserver?.disconnect();
      if (onVisibility) document.removeEventListener('visibilitychange', onVisibility);
      backend!.destroy();
    },
    requestRedraw() {
      // Only meaningful under reduced motion; the live loop already repaints.
      if (destroyed || lost || !reducedMotion || redrawQueued) return;
      redrawQueued = true;
      requestAnimationFrame((ts) => {
        redrawQueued = false;
        drawFrame(ts);
      });
    },
  };
}
