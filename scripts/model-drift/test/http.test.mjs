// fal's Platform API answers a burst of `expand=openapi-3.0` requests with
// HTTP 429 (seen 2026-11-04, once fal's carried endpoints passed a hundred): a
// source is asked again after the server's Retry-After, so one busy minute
// doesn't fail the weekly run.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { httpFetch } from '../lib.mjs';

function answering(statuses) {
  const seen = [];
  const fake = async (url) => {
    seen.push(url);
    const status = statuses[Math.min(seen.length, statuses.length) - 1];
    return new Response(status === 200 ? 'ok' : 'slow down', { status, headers: { 'retry-after': '0', 'content-type': 'text/plain' } });
  };
  return { seen, fake };
}

test('a 429 is asked again after its Retry-After, up to three times', async () => {
  const real = globalThis.fetch;
  const { seen, fake } = answering([429, 429, 429, 200]);
  globalThis.fetch = fake;
  try {
    const out = await httpFetch('https://api.example/v1/models');
    assert.equal(out.status, 200);
    assert.equal(out.body, 'ok');
    assert.equal(seen.length, 4);
  } finally {
    globalThis.fetch = real;
  }
});

test('a source still rate-limited after three retries answers its 429', async () => {
  const real = globalThis.fetch;
  const { seen, fake } = answering([429]);
  globalThis.fetch = fake;
  try {
    const out = await httpFetch('https://api.example/v1/models');
    assert.equal(out.status, 429);
    assert.equal(seen.length, 4);
  } finally {
    globalThis.fetch = real;
  }
});

test('a 404 is answered at once', async () => {
  const real = globalThis.fetch;
  const { seen, fake } = answering([404]);
  globalThis.fetch = fake;
  try {
    const out = await httpFetch('https://api.example/v1/models');
    assert.equal(out.status, 404);
    assert.equal(seen.length, 1);
  } finally {
    globalThis.fetch = real;
  }
});
