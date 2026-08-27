import type { ParamSpec, ImageGenerationRequest, Model3dGenerationRequest, Model3dAsset } from '@litegen/sdk';

export type Availability = 'live' | 'mock' | 'setup';

/** One row in the unified parameter panel. */
export interface MergedParam {
  name: string;        // API param key, e.g. "size", "style", "steps"
  spec: ParamSpec;     // merged/representative spec used to render one control
  models: string[];    // model ids that declare this param
}

/** Values the user has set in the unified panel. */
export interface SharedFormState {
  prompt: string;
  n: number;
  strict: boolean;
  seed: string;                        // "" = random
  params: Record<string, unknown>;     // keyed by MergedParam.name
}

/** `polling` is distinct from `running`: the request has been ACCEPTED and we
 *  are now waiting on a server-side job, so the tile can show real progress. */
export type TileStatus = 'queued' | 'running' | 'polling' | 'done' | 'error';

/** One result cell (model × index). */
export interface ResultTileState {
  key: string;                         // `${modelId}#${index}`
  modelId: string;
  index: number;
  status: TileStatus;
  /** Which family this tile is rendering — decides the tile component. */
  mediaType: 'image' | 'model3d';
  request: ImageGenerationRequest | Model3dGenerationRequest;
  b64_json?: string | null;
  url?: string | null;
  /** 3D only: the re-hosted asset list from the completed job. */
  assets?: Model3dAsset[];
  /** 0–100, populated while `polling`. */
  progress?: number;
  costUsd?: number;
  latencyMs?: number;
  error?: string;
}
