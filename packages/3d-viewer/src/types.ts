// The words the viewer is spoken to in. They are the converter package's
// (@litegen/3d-converter), spelled out here so a client without the
// converter, the dashboard, can use the viewer too. The converter tool's
// src/viewer-types.test.ts pins each one equal to the converter's.

/** Every container the viewer draws, each through three.js's own loader. */
export const VIEWER_FORMATS = ['glb', 'gltf', 'obj', 'stl', 'ply', '3mf', 'dae', 'fbx', 'usdz', 'usda', 'usdc'] as const;

export type ViewerFormat = (typeof VIEWER_FORMATS)[number];

/** Whether the viewer has a loader for `format` (a plain string from an API). */
export function isViewerFormat(format: unknown): format is ViewerFormat {
  return typeof format === 'string' && (VIEWER_FORMATS as readonly string[]).includes(format);
}

/** The part a file plays in a model: the model itself, the OBJ's material
 *  library, a glTF buffer, a texture, a USD layer, or none. */
export type ViewerFileRole = 'model' | 'material_library' | 'buffer' | 'texture' | 'layer' | 'unused';

/** The frame a file stores its coordinates in:
 *  - `y_up` and `z_up` by the format's own specification (glTF; 3MF);
 *  - `declared` when the file names its own axis (Collada, FBX, USD);
 *  - `caller` when the format has none, and the writer's `up` says (STL,
 *    OBJ, PLY). */
export type StoredFrame = 'y_up' | 'z_up' | 'declared' | 'caller';

export type UpAxis = 'y' | 'z';

/** One file of a model, in memory. */
export interface ViewerFile {
  /** Its path within the model, `/`-separated. */
  name: string;
  data: Uint8Array;
  role?: ViewerFileRole;
}
