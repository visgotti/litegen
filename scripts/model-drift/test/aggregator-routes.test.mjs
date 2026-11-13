// The user, 2026-11-04: "no even if we have the models/vendors already idc
// like how litellm can still use openrouter etc we want to be like that". A
// hub's route is carried even where litegen calls the vendor direct, so "we
// carry it direct" is never a reason to skip a model.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadDefinitions } from '../lib.mjs';

const DIR = join(dirname(fileURLToPath(import.meta.url)), '..', 'providers');
const DIRECT = /carried direct|carry the originals direct|integrate [\w ]+ direct|calls that vendor direct|direct covers|direct is our [\w.]+ route/i;
// Definitions whose direct-route skips are still to be reversed (the backlog in
// .superpowers/sdd/2026-11-04-tokenhub-bedrock-fal/progress.md). Remove a name
// once its skips are carried.
const PENDING = new Set(['hunyuan']);

test('no drift definition skips a model because litegen carries it direct', async () => {
  for (const def of await loadDefinitions(DIR)) {
    const direct = (def.acknowledged ?? []).filter((a) => DIRECT.test(a.reason ?? ''));
    if (PENDING.has(def.provider)) {
      assert.ok(direct.length > 0, `${def.provider} has no direct-route skip left: remove it from PENDING`);
    } else {
      assert.deepEqual(direct.map((a) => a.id ?? String(a.pattern)), [], `${def.provider}: carry these as routes, or skip them for a real reason`);
    }
  }
});
