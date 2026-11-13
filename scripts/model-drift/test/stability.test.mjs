// Stability's v1 API (SDXL 1.0) is read from the markdown twins of its
// reference pages since docs.stability.ai/openapi-v1.json went behind a login
// (2026-11-09). Each twin embeds its one operation as an OpenAPI spec in a
// ````yaml fence whose info string names the spec file and the operation.
import { test } from 'node:test';
import assert from 'node:assert/strict';

import stability from '../providers/stability.mjs';

const source = (key) => stability.sources.find((s) => s.key === key);
const ours = Object.values(stability.models).flat();

const page = (method, path, operation) => `> ## Documentation Index
> Fetch the complete documentation index at: https://docs.stability.ai/llms.txt

# Text-to-image

> Generate an image from a text prompt.
Use \`stable-diffusion-xl-1024-v1-0\` as the \`engine_id\` of your request.

\`\`\`\`yaml /openapi-v1.json ${method} ${path}
openapi: 3.0.1
info:
  title: Stability.ai REST API
  version: v1
paths:
  ${path}:
    ${method}:
      parameters:
      - $ref: '#/components/parameters/engineID'
${operation}
components:
  parameters:
    engineID:
      name: engine_id
      in: path
      required: true
      example: stable-diffusion-xl-1024-v1-0
      schema:
        type: string
\`\`\`\`

\`\`\`python
engine_id = "esrgan-v1-x2plus"
\`\`\`
`;

const TEXT_TO_IMAGE = page('post', '/v1/generation/{engine_id}/text-to-image', [
  '      requestBody:',
  '        content:',
  '          application/json:',
  '            schema:',
  '              required:',
  '              - text_prompts',
  '              properties:',
  '                cfg_scale:',
  '                  description: How strictly the diffusion process adheres to the prompt text',
  '                  type: number',
  '                  minimum: 0',
  '                  maximum: 35',
  '                  default: 7',
].join('\n'));

test('a v1 page yields the engine ids its operation names and a typings excerpt of its request body', () => {
  const src = source('v1-text-to-image');
  assert.deepEqual(src.extract(TEXT_TO_IMAGE), ['stable-diffusion-xl-1024-v1-0']);
  const snap = src.snapshot(TEXT_TO_IMAGE, '', { ours });
  assert.match(snap, /^# v1 engine_id values the page names\nstable-diffusion-xl-1024-v1-0\n/);
  assert.match(snap, /"POST \/v1\/generation\/\{engine_id\}\/text-to-image \(application\/json\)"/);
  assert.match(snap, /"maximum": 35/);
  assert.doesNotMatch(snap, /How strictly|esrgan/, 'prose and the code samples outside the spec are not typings');
});

test('a v1 page that no longer embeds its operation fails instead of yielding nothing', () => {
  assert.throws(() => source('v1-masking').extract(TEXT_TO_IMAGE), /embeds no POST \/v1\/generation\/\{engine_id\}\/image-to-image\/masking operation/);
  assert.throws(() => source('v1-text-to-image').extract('# Text-to-image\n\nThe API reference has moved.\n'), /YAML/);
});

test('the engines list is a GET: its snapshot is the engine ids and the endpoint, with no request body', () => {
  const md = page('get', '/v1/engines/list', ['      responses:', "        '200':", '          description: OK'].join('\n'));
  const src = source('v1-engines');
  assert.deepEqual(src.extract(md), ['stable-diffusion-xl-1024-v1-0']);
  assert.equal(src.snapshot(md, '', { ours }), '# v1 engine_id values the page names\nstable-diffusion-xl-1024-v1-0\n\n# GET /v1/engines/list\n');
});
