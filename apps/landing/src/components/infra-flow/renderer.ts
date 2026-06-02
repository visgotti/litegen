/**
 * WebGPU renderer for the infrastructure-flow visual.
 *
 * Responsibilities (and nothing else — the React component owns the DOM and the
 * interaction state):
 *   - feature-detect WebGPU and set up device / pipeline / uniform buffer,
 *   - each frame, read the live on-screen rect of every node relative to the
 *     stage, ease per-node hover focus and the cursor, pack the uniform, draw,
 *   - tear everything down on `destroy()`.
 *
 * The caller passes live, mutable `pointer` and `focus` objects; the renderer
 * reads them every frame without any per-frame allocation or React involvement.
 *
 * Returns `null` when WebGPU is unavailable so the component can fall back to
 * the static SVG diagram.
 */
import { FLOW_WGSL } from './shader';

/** Normalized cursor position within the stage (0..1), mutated by the caller. */
export interface PointerState {
  x: number;
  y: number;
  inside: boolean;
}

/** A node the renderer should track. Order: app, gateway, then providers. */
export interface FlowHandle {
  id: string;
  el: HTMLElement;
  role: 'app' | 'gateway' | 'provider';
  kind?: 'image' | 'video';
  /** Input-vocab bitmask (text=1|image=2|multi=4); 0 for app/gateway. */
  inputsMask?: number;
  /** Output-modality bitmask (image=1|video=2); varies the returning media tile. */
  outputsMask?: number;
}

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
   * Aborts a half-finished setup. Because device/adapter acquisition is async,
   * a fast unmount (or StrictMode's mount→cleanup→mount) could otherwise create
   * a GPU device and reconfigure the shared canvas *after* teardown. When the
   * signal is already aborted at any await boundary we bail and clean up.
   */
  signal?: AbortSignal;
  /**
   * Called when the GPU device is lost unexpectedly (driver reset, browser
   * reclaiming the device) — i.e. NOT as part of our own teardown. Lets the
   * component fall back to the static SVG instead of freezing on a dead canvas.
   */
  onLost?: () => void;
}

export interface FlowRenderer {
  destroy(): void;
  /**
   * Request a single repaint. A no-op while the animation loop is running
   * (under normal motion); under reduced motion — where there is no rAF loop —
   * this is how hover focus changes get reflected. Coalesced to one frame.
   */
  requestRedraw(): void;
}

// Mirrors the std140 uniform layout in shader.ts — KEEP IN SYNC with the
// `array<vec4<f32>, N>` sizes there. There are 20 live nodes (app + gateway +
// 18 providers); the cap is 24 so every provider gets links/halos/media (an
// earlier cap of 8 silently dropped 12 providers via the slice below) with a
// little headroom. Bump this *and* both WGSL arrays together or the std140
// layout desyncs and the uniform reads garbage.
const MAX_NODES = 24;
// res+ctl+mouse(12) + geom[N] + meta[N] + flags[N]. KEEP IN SYNC with the three
// array<vec4<f32>, N> sizes in shader.ts — bump all of them together or the
// std140 layout desyncs and the uniform reads garbage.
const UNIFORM_FLOATS = 12 + MAX_NODES * 4 * 3;
const GEOM_BASE = 12;
const META_BASE = 12 + MAX_NODES * 4;
const FLAGS_BASE = 12 + MAX_NODES * 4 * 2;
const DPR_CAP = 2;
const EASE = 0.12;

function roleNum(role: FlowHandle['role']): number {
  if (role === 'app') return 0;
  if (role === 'gateway') return 1;
  return 2;
}

export async function createFlowRenderer(opts: RendererOpts): Promise<FlowRenderer | null> {
  if (typeof navigator === 'undefined' || !navigator.gpu) return null;
  if (opts.signal?.aborted) return null;

  let adapter: GPUAdapter | null = null;
  try {
    adapter = await navigator.gpu.requestAdapter();
  } catch {
    return null;
  }
  if (!adapter || opts.signal?.aborted) return null;

  let device: GPUDevice;
  try {
    device = await adapter.requestDevice();
  } catch {
    return null;
  }

  // Setup was cancelled while we were awaiting the device — don't touch the
  // (possibly remounted) canvas; just release the device we just acquired.
  if (opts.signal?.aborted) {
    device.destroy?.();
    return null;
  }

  const ctx = opts.canvas.getContext('webgpu');
  if (!ctx) {
    device.destroy?.();
    return null;
  }

  const format = navigator.gpu.getPreferredCanvasFormat();
  ctx.configure({ device, format, alphaMode: 'opaque' });

  const shaderModule = device.createShaderModule({ code: FLOW_WGSL });
  const pipeline = device.createRenderPipeline({
    layout: 'auto',
    vertex: { module: shaderModule, entryPoint: 'vs' },
    fragment: { module: shaderModule, entryPoint: 'fs', targets: [{ format }] },
    primitive: { topology: 'triangle-list' },
  });

  const data = new Float32Array(UNIFORM_FLOATS);
  const uniformBuffer = device.createBuffer({
    size: data.byteLength,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
  });
  const bindGroup = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [{ binding: 0, resource: { buffer: uniformBuffer } }],
  });

  const { canvas, stage, nodes, pointer, focus, reducedMotion } = opts;
  const nodeList = nodes.slice(0, MAX_NODES);

  // Eased state lives here so it survives between frames.
  const easedFocus = new Map<string, number>(nodeList.map((n) => [n.id, 0]));
  const easedPointer = { x: pointer.x, y: pointer.y };

  let destroyed = false;
  let raf = 0;
  let lost = false;
  // Surface only *unexpected* losses. A 'destroyed' reason (or a loss that
  // races our own destroy()) is our teardown and must not trigger the fallback.
  device.lost.then((info) => {
    lost = true;
    if (!destroyed && info.reason !== 'destroyed') opts.onLost?.();
  });

  const t0 = typeof performance !== 'undefined' ? performance.now() : 0;

  function ensureSize(rect: DOMRect): number {
    const dpr = Math.min(typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1, DPR_CAP);
    const w = Math.max(1, Math.round(rect.width * dpr));
    const h = Math.max(1, Math.round(rect.height * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    return dpr;
  }

  function renderFrame(now: number) {
    if (destroyed || lost) return;

    const rect = stage.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) {
      if (!reducedMotion) raf = requestAnimationFrame(renderFrame);
      return;
    }

    const dpr = ensureSize(rect);
    const t = (now - t0) / 1000;

    // ease cursor (skip drift under reduced motion)
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
      // shader can draw a rounded-box halo that hugs rectangular cards instead
      // of a circle that overshoots their short axis.
      const m = META_BASE + i * 4;
      data[m] = roleNum(node.role);
      data[m + 1] = node.kind === 'video' ? 1 : 0;
      data[m + 2] = b.width / 2 / rect.width;
      data[m + 3] = b.height / 2 / rect.height;

      // Per-node flags: .x input-vocab bitmask (app/gateway → 0 → "text only");
      // .y output-modality bitmask (image=1|video=2) for the returning media.
      data[FLAGS_BASE + i * 4] = node.inputsMask ?? 0;
      data[FLAGS_BASE + i * 4 + 1] = node.outputsMask ?? 0;
    }

    device.queue.writeBuffer(uniformBuffer, 0, data);

    const encoder = device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
      colorAttachments: [
        {
          view: ctx!.getCurrentTexture().createView(),
          clearValue: { r: 0.043, g: 0.047, b: 0.071, a: 1 },
          loadOp: 'clear',
          storeOp: 'store',
        },
      ],
    });
    pass.setPipeline(pipeline);
    pass.setBindGroup(0, bindGroup);
    pass.draw(3);
    pass.end();
    device.queue.submit([encoder.finish()]);

    if (!reducedMotion) raf = requestAnimationFrame(renderFrame);
  }

  // Reduced motion: render a single static frame and re-render only on resize.
  let resizeObserver: ResizeObserver | undefined;
  if (reducedMotion) {
    renderFrame(t0);
    if (typeof ResizeObserver !== 'undefined') {
      resizeObserver = new ResizeObserver(() => renderFrame(t0));
      resizeObserver.observe(stage);
    }
  } else {
    raf = requestAnimationFrame(renderFrame);
  }

  let redrawQueued = false;

  return {
    destroy() {
      destroyed = true;
      if (raf) cancelAnimationFrame(raf);
      resizeObserver?.disconnect();
      try {
        uniformBuffer.destroy();
        device.destroy?.();
      } catch {
        /* device may already be gone */
      }
    },
    requestRedraw() {
      // Only meaningful under reduced motion; the live loop already repaints.
      if (destroyed || lost || !reducedMotion || redrawQueued) return;
      redrawQueued = true;
      requestAnimationFrame((ts) => {
        redrawQueued = false;
        renderFrame(ts);
      });
    },
  };
}
