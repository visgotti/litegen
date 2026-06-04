/**
 * WebGPU draw backend for the infra-flow visual.
 *
 * Owns the device / pipeline / uniform buffer and nothing else: the engine
 * (renderer.ts) packs the std140 `data` each frame and calls draw(). Returns
 * null when WebGPU is unavailable / setup is aborted, so the engine can fall
 * through to the WebGL2 backend.
 */
import { FLOW_WGSL } from './shader';
import { UNIFORM_FLOATS, type BackendInit, type FlowBackend } from './types';

// Panel base colour (matches the CSS); the opaque fullscreen triangle overwrites
// every pixel, so this only shows for the one cleared frame before the first draw.
const CLEAR = { r: 0.043, g: 0.047, b: 0.071, a: 1 } as const;

export async function createWebGPUBackend(init: BackendInit): Promise<FlowBackend | null> {
  const { canvas, lowPower, onLost, signal } = init;
  if (typeof navigator === 'undefined' || !navigator.gpu) return null;
  if (signal?.aborted) return null;

  let adapter: GPUAdapter | null = null;
  try {
    adapter = await navigator.gpu.requestAdapter(
      lowPower ? { powerPreference: 'low-power' } : undefined,
    );
  } catch {
    return null;
  }
  if (!adapter || signal?.aborted) return null;

  let device: GPUDevice;
  try {
    device = await adapter.requestDevice();
  } catch {
    return null;
  }

  // Setup was cancelled while awaiting the device — release it and bail without
  // touching the (possibly remounted) canvas.
  if (signal?.aborted) {
    device.destroy?.();
    return null;
  }

  const ctx = canvas.getContext('webgpu');
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

  const uniformBuffer = device.createBuffer({
    size: UNIFORM_FLOATS * 4,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
  });
  const bindGroup = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [{ binding: 0, resource: { buffer: uniformBuffer } }],
  });

  let destroyed = false;
  let lost = false;
  // Surface only *unexpected* losses — a 'destroyed' reason (or a loss racing our
  // own destroy()) is our teardown and must not trigger the SVG fallback.
  device.lost.then((info) => {
    lost = true;
    if (!destroyed && info.reason !== 'destroyed') onLost();
  });

  return {
    kind: 'webgpu',
    resize() {
      // The configured context tracks canvas.width/height automatically; the next
      // getCurrentTexture() is already the new size. Nothing to do.
    },
    draw(data: Float32Array) {
      if (destroyed || lost) return;
      // The engine's buffer is always ArrayBuffer-backed; narrow away the generic
      // ArrayBufferLike so writeBuffer (which rejects SharedArrayBuffer) is happy.
      device.queue.writeBuffer(uniformBuffer, 0, data as Float32Array<ArrayBuffer>);
      const encoder = device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [
          {
            view: ctx.getCurrentTexture().createView(),
            clearValue: CLEAR,
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
    },
    destroy() {
      destroyed = true;
      try {
        uniformBuffer.destroy();
        device.destroy?.();
      } catch {
        /* device may already be gone */
      }
    },
  };
}
