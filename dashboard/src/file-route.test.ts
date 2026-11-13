import { describe, it, expect } from 'vitest';
import { fileRouteOf, isFileRoute } from './file-route';

describe('isFileRoute', () => {
  it("names litegen's file route, which wants the dashboard's credentials", () => {
    for (const url of [
      'http://127.0.0.1:4000/v1/generations/litegen-3d-1f2e/assets/0',
      'https://app.litegen.ai/api/v1/generations/litegen-img-9/assets/12',
      // A fragment is never sent on the wire: the same file.
      'https://app.litegen.ai/api/v1/generations/g/assets/3#retry-2',
    ]) {
      expect(isFileRoute(url), url).toBe(true);
    }
  });

  it('leaves every other URL to the element', () => {
    for (const url of [
      // litegen's public asset route, plain or signed
      'http://127.0.0.1:4000/v1/models3d/assets/litegen/3d/g/model.glb',
      'http://127.0.0.1:4000/v1/models3d/assets/litegen/3d/g/model.glb?expires=1&signature=ab',
      // a presigned bucket URL that happens to contain the words
      'https://bucket.s3.amazonaws.com/v1/generations/g/assets/0.png?X-Amz-Signature=x',
      // the asset list, and a non-numeric index
      'https://app.litegen.ai/api/v1/generations/g/assets',
      'https://app.litegen.ai/api/v1/generations/g/assets/x',
      'data:image/png;base64,AAAA',
      'blob:http://127.0.0.1:5174/1d4e',
      '',
    ]) {
      expect(isFileRoute(url), url).toBe(false);
    }
  });
});

describe('fileRouteOf', () => {
  it('reads the generation and the index the route names', () => {
    expect(fileRouteOf('https://app.litegen.ai/api/v1/generations/litegen-3d-1f2e/assets/12#retry-1')).toEqual({ generationId: 'litegen-3d-1f2e', index: 12 });
    expect(fileRouteOf('http://127.0.0.1:4000/v1/models3d/assets/litegen/3d/g/model.glb')).toBeNull();
  });
});
