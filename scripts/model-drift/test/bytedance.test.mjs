// BytePlus restructured its model list and pricing pages (2026-10): a
// `## <family>` section per medium, each split into `###` tables, an "Also
// supports `id`" second id, and "new" badges glued to bold names
// (ByteDance/MiniMax/Vidu audit D1–D4, 2026-10-30).
import { test } from 'node:test';
import assert from 'node:assert/strict';

import bytedance from '../providers/bytedance.mjs';

const page = (md) => ({ Result: { MDContent: md } });
const source = (key) => bytedance.sources.find((s) => s.key === key);
const ours = Object.values(bytedance.models).flat();

const MODEL_LIST = `## Text and code generation

|Model ID |Context |
|---|---|
|[seed-2-0-260128](https://x) |256k |

## Video generation

### Capabilities

|Model ID |Text\\-to\\-video |First\\-frame\\-to\\-video |
|---|---|---|
|[dreamina-seedance-2-0-mini-260615](https://x) |Yes |Yes |
|[seedance-1-0-pro-250528](https://x) |Yes |Yes |

### Limits

|Model ID |Max RPM |
|---|---|
|[seedance-1-0-pro-250528](https://x) |600 |

Retiring models

|Model ID |Recommended replacement |
|---|---|
|[seedance-1-5-pro-251215](https://x) |\`dreamina-seedance-2-0-mini-260615\` |

## Image generation

### Capabilities

|Model ID |Layer decomposition |
|---|---|
|[dola-seedream-5-0-flash-260915](https://x) |Yes |
|[seedream-5-0-260128](https://x)<br><br>Also supports \`seedream-5-0-lite-260128\` |No |

## 3D generation

|Model ID |Output |
|---|---|
|[hyper3d-gen2-260112](https://x) |glb |
`;

test("the model list is read from each medium's tables, second ids included", () => {
  const ids = source('model-list').extract(page(MODEL_LIST));
  assert.deepEqual(
    [...new Set(ids)].sort(),
    ['dola-seedream-5-0-flash-260915', 'dreamina-seedance-2-0-mini-260615', 'seedance-1-0-pro-250528', 'seedance-1-5-pro-251215', 'seedream-5-0-260128', 'seedream-5-0-lite-260128'],
  );
  const snap = source('model-list').snapshot(page(MODEL_LIST), '', { ours });
  assert.match(snap, /dreamina-seedance-2-0-mini-260615 \| Yes \| Yes/);
  assert.match(snap, /seedream-5-0-lite-260128/);
  assert.doesNotMatch(snap, /600/, 'rate limits are not typings');
});

const PRICING = `## Large language models

### Pricing

|Model |Input |
|---|---|
|seed\\-2\\-0\\-260128 |$1 |

## Video generation models

### Pricing

**Input without video**

|Model |Online inference |||Offline inference |
|---|---|---|---|---|
| |480p and 720p |1080p |4K | |
|dreamina\\-seedance\\-2\\-0\\-mini\\-260615 |$3.5 |Not supported |Not supported |Not supported |
|seedance\\-1\\-0\\-pro\\-250528 |$2.5 |||$1.25 |

### Billing examples

#### Seedance 1.0 Pro

|Resolution |Ratio |Price |
|---|---|---|
|1080p |16:9 |$0.61 |

## Image generation models

### Pricing

|Model ID |Input image |Output image |
|---|---|---|
|\`seedream-4-0-250828\` |Free |$0.03 |
|\`dola-seedream-5-0-flash-260915\` |Free |$0.018 |
|\`seededit-3-0-i2i-250628\` |Free |$0.03 |
`;

test("prices are read from each medium's own Pricing section", () => {
  const ids = source('pricing').extract(page(PRICING));
  assert.deepEqual([...ids].sort(), [
    'dola-seedream-5-0-flash-260915', 'dreamina-seedance-2-0-mini-260615', 'seedance-1-0-pro-250528', 'seededit-3-0-i2i-250628', 'seedream-4-0-250828',
  ]);
  const snap = source('pricing').snapshot(page(PRICING), '', { ours });
  assert.match(snap, /seedance-1-0-pro-250528 \| \$2\.5/);
  assert.match(snap, /\$0\.61/, 'the billing example our flat price comes from');
  assert.match(snap, /seedream-4-0-250828 \| Free \| \$0\.03/);
  // Seedream 5.0 Flash is carried since 2026-11-03; SeedEdit is not.
  assert.match(snap, /dola-seedream-5-0-flash-260915 \| Free \| \$0\.018/);
  assert.doesNotMatch(snap, /seed-2-0-260128|seededit/);
});

test('a "new" badge glued to a bold name is not part of the name', () => {
  const md = '### Request body\n\n**layer_decompositionnew** `boolean` `Optional`\n\n**model** `string` `Required`\n';
  assert.deepEqual(source('video-api').extract(page(md)), ['layer_decomposition', 'model']);
});

// 2026-11-09: the image page renamed its request-body heading to "### Body
// parameters" (under "## Request parameters") and appends a " | Title" to
// each signature; the video page kept "### Request body".
const IMAGE_API = `## Request parameters

### Body parameters

**prompt** \`string\`  |  Prompt

**Image generation scenario ** **\`Required\`**

**model** \`string\` \`Required\`  |  Model ID

## Response parameters

**model** \`string\`  |  Model ID
`;

test('the image page is read from its "### Body parameters" section, the video page from "### Request body"', () => {
  assert.deepEqual(source('image-api').extract(page(IMAGE_API)), ['prompt', 'model']);
  const snap = source('image-api').snapshot(page(IMAGE_API), '', { ours });
  assert.equal(snap, 'prompt `string`\n  [Image generation scenario] Required\nmodel `string` `Required`\n');
  assert.throws(() => source('video-api').extract(page(IMAGE_API)), /"### Request body" not found/);
  assert.throws(() => source('image-api').extract(page(IMAGE_API.replace('### Body parameters', '### Request body'))), /"### Body parameters" not found/);
});
