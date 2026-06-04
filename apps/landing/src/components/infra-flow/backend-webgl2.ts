/**
 * WebGL2 draw backend — the fallback used when WebGPU is unavailable (most
 * mobile Safari, older desktops). Draws the GLSL ES 3.00 port of the shader from
 * the same std140 uniform buffer the WebGPU path uses, so the output matches.
 *
 * Returns null if WebGL2 is missing or the program fails to compile/link, so the
 * engine can fall through to the static SVG diagram.
 */
import { FLOW_GLSL_FS, FLOW_GLSL_VS } from './shader-glsl';
import { UNIFORM_FLOATS, type BackendInit, type FlowBackend } from './types';

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader | null {
  const sh = gl.createShader(type);
  if (!sh) return null;
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    // Surfacing this helps diagnose a device-specific GLSL issue; the engine
    // still falls back to SVG so users never see a blank canvas.
    console.warn('[infra-flow] WebGL2 shader compile failed:', gl.getShaderInfoLog(sh));
    gl.deleteShader(sh);
    return null;
  }
  return sh;
}

export function createWebGL2Backend(init: BackendInit): FlowBackend | null {
  const { canvas, lowPower, onLost } = init;

  const gl = canvas.getContext('webgl2', {
    alpha: false,
    antialias: false,
    depth: false,
    stencil: false,
    premultipliedAlpha: false,
    powerPreference: lowPower ? 'low-power' : 'high-performance',
  });
  if (!gl) return null;

  const vs = compile(gl, gl.VERTEX_SHADER, FLOW_GLSL_VS);
  const fs = compile(gl, gl.FRAGMENT_SHADER, FLOW_GLSL_FS);
  if (!vs || !fs) return null;

  const program = gl.createProgram();
  if (!program) return null;
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  // Shaders are linked in; safe to drop the individual objects.
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.warn('[infra-flow] WebGL2 program link failed:', gl.getProgramInfoLog(program));
    gl.deleteProgram(program);
    return null;
  }

  // Bind the std140 uniform block to binding point 0.
  const blockIndex = gl.getUniformBlockIndex(program, 'U');
  if (blockIndex === gl.INVALID_INDEX) {
    gl.deleteProgram(program);
    return null;
  }
  gl.uniformBlockBinding(program, blockIndex, 0);

  const ubo = gl.createBuffer();
  gl.bindBuffer(gl.UNIFORM_BUFFER, ubo);
  gl.bufferData(gl.UNIFORM_BUFFER, UNIFORM_FLOATS * 4, gl.DYNAMIC_DRAW);
  gl.bindBufferBase(gl.UNIFORM_BUFFER, 0, ubo);

  // A bound VAO is required to draw; the vertex shader uses gl_VertexID, so it
  // needs no attributes.
  const vao = gl.createVertexArray();

  gl.disable(gl.DEPTH_TEST);
  gl.disable(gl.BLEND);
  gl.disable(gl.CULL_FACE);

  let destroyed = false;
  let lost = false;
  const onContextLost = (e: Event) => {
    e.preventDefault(); // allow a potential restore, but we fall back to SVG now
    lost = true;
    if (!destroyed) onLost();
  };
  canvas.addEventListener('webglcontextlost', onContextLost as EventListener, false);

  return {
    kind: 'webgl2',
    resize(width: number, height: number) {
      if (destroyed || lost) return;
      gl.viewport(0, 0, width, height);
    },
    draw(data: Float32Array) {
      if (destroyed || lost || gl.isContextLost()) return;
      gl.useProgram(program);
      gl.bindVertexArray(vao);
      gl.bindBuffer(gl.UNIFORM_BUFFER, ubo);
      gl.bufferSubData(gl.UNIFORM_BUFFER, 0, data);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    },
    destroy() {
      destroyed = true;
      canvas.removeEventListener('webglcontextlost', onContextLost as EventListener, false);
      try {
        gl.deleteBuffer(ubo);
        gl.deleteVertexArray(vao);
        gl.deleteProgram(program);
        // Free the GPU context promptly (esp. helpful on mobile).
        gl.getExtension('WEBGL_lose_context')?.loseContext();
      } catch {
        /* context may already be gone */
      }
    },
  };
}
