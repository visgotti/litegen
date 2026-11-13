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
