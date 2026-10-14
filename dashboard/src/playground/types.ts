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
  /**
   * Only the params the user has actually set, keyed by MergedParam.name. An
   * untouched param is absent and is never sent, so each model applies its
   * own default — a value the client invented (an int's minimum, an enum's
   * first member) would silently override it.
   */
  params: Record<string, unknown>;
  /** Reference images, each tagged with the role it fills. */
  refs: RefEntry[];
  /** Raw text per provider-passthrough key (`extra`); parsed at send time. */
  extra: Record<string, string>;
}

/**
 * One reference image. `url` is fetched by the server (or the provider);
 * `base64` carries an uploaded file inline as a `data:` URL.
 */
export interface RefEntry {
  id: string;                          // stable React key
  role: string;                        // e.g. "init", "view-front"
  type: 'url' | 'base64';
  value: string;
  /** The uploaded file's name, shown instead of a multi-megabyte data URL. */
  name?: string;
}

/** One reference-image role across the selected models. */
export interface MergedRole {
  role: string;
  /** The strictest per-model cap: no selected model is sent more than it accepts. */
  maxCount: number;
  /** The most any requiring model needs. */
  minCount: number;
  /** Models that declare the role. */
  models: string[];
  /** Models that will reject the request without enough images in this role. */
  requiredBy: string[];
}

/** One provider-passthrough key (`extra_allowlist`) across the selected models. */
export interface MergedExtra {
  key: string;
  models: string[];
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
  /** What lax mode (`strict: false`) dropped instead of rejecting. */
  droppedParams?: string[];
}
