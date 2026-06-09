import type { ParamSpec, ImageGenerationRequest } from '@litegen/sdk';

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

export type TileStatus = 'queued' | 'running' | 'done' | 'error';

/** One result cell (model × index). */
export interface ResultTileState {
  key: string;                         // `${modelId}#${index}`
  modelId: string;
  index: number;
  status: TileStatus;
  request: ImageGenerationRequest;
  b64_json?: string | null;
  url?: string | null;
  costUsd?: number;
  latencyMs?: number;
  error?: string;
}
