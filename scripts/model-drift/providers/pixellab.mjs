// PixelLab v2 — native pixel art (PixelGenie L16). The OpenAPI spec the API
// itself serves: every operation is its own endpoint, `POST /v2/{operation}`,
// so the vendor id is the path after the base URL (the body has no `model`
// field). The llms.txt index lists the same endpoints, grouped.
// Mapping mirrors litegen-core/src/providers/image/pixellab.rs (one carried
// model: pixflux at /create-image-pixflux).
import { openapiExcerpt } from '../extract.mjs';

const MODELS = {
  'pixellab/pixflux': 'create-image-pixflux',
};

const ENDPOINTS = Object.values(MODELS).map((id) => `/${id}`);

// Generating and editing operations: POSTs that are not per-object or
// per-upload sub-resources (those carry a `{…}` path parameter or live under
// uploads/).
const operations = (spec) =>
  Object.entries(spec.paths)
    .filter(([path, ops]) => ops.post && !path.includes('{') && !path.startsWith('/uploads'))
    .map(([path]) => path.slice(1));

// Read 2026-10-30. Each is a PixelGenie L16 decision (directions, animation,
// tiles, edits) or a utility litegen covers elsewhere.
const NOT_CARRIED = [
  'animate-character', 'animate-pixminimax', 'animate-with-skeleton', 'animate-with-skeleton-v3', 'animate-with-text',
  'animate-with-text-v2', 'animate-with-text-v3', 'characters/animations', 'correct-pixelart', 'create-1-direction-object',
  'create-8-direction-object', 'create-character-pro', 'create-character-pro-flash', 'create-character-state', 'create-character-v3',
  'create-character-with-4-directions', 'create-character-with-8-directions', 'create-image-bitforge', 'create-image-pixen',
  'create-image-pixflux-background', 'create-image-pro-flash', 'create-isometric-tile', 'create-object-pro-flash', 'create-tiles-pro',
  'create-tileset', 'create-tileset-sidescroller', 'create-ui-asset', 'edit-animation-v2', 'edit-image', 'edit-image-pixen',
  'edit-image-pro-flash', 'edit-images-v2', 'enhance-animation-v3-prompt', 'enhance-character-v3-prompt', 'enhance-pixen-prompt',
  'estimate-skeleton', 'generate-8-rotations-v2', 'generate-8-rotations-v3', 'generate-font-pro', 'generate-image-v2', 'generate-ui-v2',
  'generate-with-style-v2', 'image-to-pixelart', 'image-to-pixelart-pro', 'image-to-pixelart-pro-flash', 'image-to-text', 'inpaint',
  'inpaint-image-pro-flash', 'inpaint-v3', 'interpolation-v2', 'lip-sync', 'map-objects', 'normal-map', 'portrait-character-pro',
  'reduce-colors', 'remove-background', 'resize', 'rotate', 'talking-gif', 'tilesets', 'tilesets-sidescroller', 'transfer-outfit-v2',
  'unzoom', 'vfx', 'vocal-animation',
];

export default {
  provider: 'pixellab',
  models: MODELS,
  sources: [
    {
      key: 'openapi',
      url: 'https://api.pixellab.ai/v2/openapi.json',
      expect: 'json',
      extract: operations,
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
  ],
  acknowledged: NOT_CARRIED.map((id) => ({
    id,
    reason: 'not carried yet (read 2026-10-30): directions, animation, tiles, edits and utilities wait on the PixelGenie L16 vendor decision',
  })),
};
