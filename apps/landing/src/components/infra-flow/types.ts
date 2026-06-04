/**
 * Shared contracts + uniform layout for the infrastructure-flow renderer.
 *
 * The visual has two interchangeable GPU backends (WebGPU, with a WebGL2
 * fallback) that draw the exact same fullscreen-triangle fragment shader from
 * the exact same std140 uniform buffer. This module is the single source of
 * truth for the buffer layout and the backend interface so the two can never
 * drift apart.
 */

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

// ---- std140 uniform layout (mirrored by shader.ts / shader-glsl.ts) ---------
// There are 20 live nodes (app + gateway + 18 providers); the cap is 24 so every
// provider gets links/halos/media with headroom. Bump this *and* the three
// `array<vec4, N>` sizes in BOTH shaders together or the std140 layout desyncs.
export const MAX_NODES = 24;
// res+ctl+mouse(12 floats) + geom[N] + meta[N] + flags[N], each vec4 = 4 floats.
export const UNIFORM_FLOATS = 12 + MAX_NODES * 4 * 3;
export const GEOM_BASE = 12;
export const META_BASE = 12 + MAX_NODES * 4;
export const FLAGS_BASE = 12 + MAX_NODES * 4 * 2;

/** Options shared by both backend factories. */
export interface BackendInit {
  canvas: HTMLCanvasElement;
  /** Prefer the low-power GPU (mobile / low-end tier). */
  lowPower: boolean;
  /** Fires on *unexpected* context/device loss (not our own teardown). */
  onLost: () => void;
  /** Aborts a half-finished async setup (WebGPU device acquisition). */
  signal?: AbortSignal;
}

/**
 * A draw backend. The engine owns the canvas size, per-frame easing and uniform
 * packing; a backend only uploads the packed `data` and issues the draw.
 */
export interface FlowBackend {
  readonly kind: 'webgpu' | 'webgl2';
  /** Canvas pixel size changed — update viewport/textures as needed. */
  resize(width: number, height: number): void;
  /** Upload the std140 uniform `data` (UNIFORM_FLOATS long) and draw one frame. */
  draw(data: Float32Array): void;
  destroy(): void;
}
