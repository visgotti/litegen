// fal's carried-endpoint sources, on the real definition: a deprecated endpoint
// is still served, and a shutdown date fal announces later must show up.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fal from '../providers/fal.mjs';
import { evaluateSource } from '../lib.mjs';

const source = fal.sources.find((s) => s.key === 'carried-image');
const ours = Object.values(fal.models).flat();
const answer = (models) => ({ status: 200, contentType: 'application/json', body: JSON.stringify({ models }) });

test("fal's registry status 'deprecated' reads as still served, not as gone", () => {
  // Trimmed from GET https://api.fal.ai/v1/models?endpoint_id=fal-ai/aura-flow (2026-11-11):
  // "status":"deprecated", "retires_at":null, "replacement":null.
  const r = evaluateSource(source, answer([
    { endpoint_id: 'fal-ai/flux-pro/v1.1', metadata: { status: 'active', category: 'text-to-image' } },
    { endpoint_id: 'fal-ai/aura-flow', metadata: { status: 'deprecated', category: 'text-to-image' }, retires_at: null, replacement: null },
  ]), { ours });
  assert.equal(r.ok, true, r.error);
  assert.deepEqual(r.ids, ['fal-ai/flux-pro/v1.1']);
  assert.deepEqual(r.deprecated, ['fal-ai/aura-flow']);
});

test("fal's carried snapshot keeps an endpoint's heading as it is while fal gives no shutdown date or replacement", () => {
  const r = evaluateSource(source, answer([
    { endpoint_id: 'fal-ai/aura-flow', metadata: { status: 'deprecated', category: 'text-to-image' }, retires_at: null, replacement: null },
  ]), { ours });
  assert.equal(r.ok, true, r.error);
  assert.match(r.snapshot, /^## fal-ai\/aura-flow — deprecated text-to-image$/m);
});

test("fal's carried source asks endpoint by endpoint when fal refuses the whole call for one unknown id, and that id reads as gone", async () => {
  // Seen 2026-11-16: GET …/v1/models?endpoint_id=fal-ai%2Faura-flow&…&expand=openapi-3.0 answers
  // 404 {"error":{"type":"not_found","message":"Endpoint(s) not found"}} for the whole chunk
  // once one id has left the registry; the other ids still answer 200 one at a time.
  const ids = new URL(source.url).searchParams.getAll('endpoint_id');
  const gone = ids[1];
  const notFound = { status: 404, contentType: 'application/json', body: '{"error":{"type":"not_found","message":"Endpoint(s) not found"}}' };
  const seen = [];
  const get = async (url) => {
    seen.push(url);
    const asked = new URL(url).searchParams.getAll('endpoint_id');
    if (asked.length !== 1 || asked[0] === gone) return notFound;
    return answer([{ endpoint_id: asked[0], metadata: { status: 'active', category: 'text-to-image' } }]);
  };
  const response = await source.fetch(source.url, get);
  assert.equal(seen.length, 1 + ids.length, 'one call for the chunk, then one per id');
  for (const url of seen.slice(1)) assert.match(url, /expand=openapi-3\.0/);
  const r = evaluateSource(source, response, { ours });
  assert.equal(r.ok, true, r.error);
  assert.deepEqual(r.ids, ids.filter((id) => id !== gone).map((id) => id.toLowerCase()).sort());
  // Gone, so `missing upstream` even while a catalog page still lists it (as
  // fal's did for fal-ai/stable-diffusion-v35-medium on 2026-11-16).
  assert.deepEqual(r.gone, [gone.toLowerCase()]);
  assert.match(r.snapshot, new RegExp(`^## ${gone.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')} — not in the model registry$`, 'm'));
});

test("fal's carried chunk whose every endpoint has left the registry reads as gone, not as a broken source", async () => {
  // Every id 404s, alone and together: fal answered (it knows none of them), so
  // the ids are missing upstream. Extracting no active or deprecated id must not
  // fail the source, or the run would re-fetch it and report a broken fetcher.
  const ids = new URL(source.url).searchParams.getAll('endpoint_id');
  const notFound = { status: 404, contentType: 'application/json', body: '{"error":{"type":"not_found","message":"Endpoint(s) not found"}}' };
  const response = await source.fetch(source.url, async () => notFound);
  const r = evaluateSource(source, response, { ours });
  assert.equal(r.ok, true, r.error);
  assert.deepEqual(r.ids, []);
  assert.deepEqual(r.deprecated, []);
  assert.deepEqual(r.gone, ids.map((id) => id.toLowerCase()).sort());
});

test("fal's carried source passes any other answer through untouched, a 404 that is not fal's unknown-endpoint error included", async () => {
  const moved = { status: 404, contentType: 'text/html', body: '<html>moved</html>' };
  let calls = 0;
  const response = await source.fetch(source.url, async () => { calls++; return moved; });
  assert.equal(calls, 1);
  assert.equal(response, moved);
});

test("fal's carried snapshot shows a shutdown date and a replacement once fal announces them", () => {
  // Under the drift lifecycle a deprecated_at row's `deprecated` entry is dropped,
  // so a later shutdown date can only surface as this snapshot changing. The
  // active endpoint keeps the chunk's ids non-empty, so the only thing this
  // test can fail on is the heading.
  const r = evaluateSource(source, answer([
    { endpoint_id: 'fal-ai/flux-pro/v1.1', metadata: { status: 'active', category: 'text-to-image' } },
    { endpoint_id: 'fal-ai/aura-flow', metadata: { status: 'deprecated', category: 'text-to-image' }, retires_at: '2027-01-31T00:00:00Z', replacement: 'fal-ai/flux-2' },
  ]), { ours });
  assert.equal(r.ok, true, r.error);
  assert.match(r.snapshot, /^## fal-ai\/aura-flow — deprecated text-to-image.*retires_at 2027-01-31T00:00:00Z.*replacement fal-ai\/flux-2$/m);
});
