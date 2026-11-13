// Ported from dashboard/src/components/ModelViewer.tsx (`hasWebGL2`,
// 123-135): whether this browser can create a WebGL 2 context, probed once.
// three.js (r183) dropped WebGL 1, so a WebGL 1 context would pass a looser
// probe and still draw nothing.
let support: boolean | undefined;

export function hasWebGL2(): boolean {
  if (support === undefined) {
    try {
      const gl = document.createElement('canvas').getContext('webgl2');
      support = gl != null;
      // Browsers cap live contexts per page: release the probe's now.
      gl?.getExtension('WEBGL_lose_context')?.loseContext();
    } catch {
      support = false;
    }
  }
  return support;
}

export const WEBGL_MESSAGE = "WebGL 2 is unavailable in this browser, so the model can't be previewed — download it instead.";
