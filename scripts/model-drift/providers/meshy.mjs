// Meshy — the official OpenAPI spec (docs.meshy.ai/openapi.json, server
// https://api.meshy.ai/openapi). Carried: rigging (POST /v1/rigging) and
// animation (POST /v1/animations). Meshy's request bodies have no `model`
// field, so the vendor ids are the endpoint paths the adapter posts to —
// mapping mirrors litegen-core/src/providers/model3d/meshy.rs `generate` —
// and "new" is scoped to POST paths in the rigging, animation and
// text-to-motion families. The animation library (the action ids
// meshy/animate offers) is snapshotted as `id key` lines so an added or
// removed clip shows as a changed snapshot.
import { openapiExcerpt } from '../extract.mjs';

const MODELS = {
  'meshy/rig': '/v1/rigging',
  'meshy/animate': '/v1/animations',
};
const FAMILY = /^\/v1\/(?:rigging|animations|text-to-motion)$/;

export default {
  provider: 'meshy',
  models: MODELS,
  sources: [
    {
      key: 'openapi',
      url: 'https://docs.meshy.ai/openapi.json',
      expect: 'json',
      extract: (spec) => Object.keys(spec.paths ?? {}).filter((p) => FAMILY.test(p) && spec.paths[p].post),
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: Object.values(MODELS), ours }) + Object.values(MODELS).sort().join('\n') + '\n',
    },
    {
      key: 'library',
      url: 'https://api.meshy.ai/web/public/animations/resources',
      expect: 'json',
      index: true,
      extract: (lib) => {
        const list = (lib.result ?? lib).list;
        if (!Array.isArray(list)) throw new Error('library has no `list`');
        return list.filter((a) => a.id >= 0).map((a) => String(a.id));
      },
      snapshot: (lib) => (lib.result ?? lib).list
        .filter((a) => a.id >= 0)
        .sort((a, b) => a.id - b.id)
        .map((a) => `${a.id} ${a.key}`)
        .join('\n') + '\n',
    },
  ],
  acknowledged: [],
  untyped: {},
};
