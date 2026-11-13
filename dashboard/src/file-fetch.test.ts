import { describe, expect, it } from 'vitest';
import { storedFileFetch, type DownloadAsset } from './file-fetch';

const ROUTE = 'https://app.litegen.test/api/v1/generations/gen-1/assets/2';

describe('storedFileFetch', () => {
  it("downloads litegen's file route through the dashboard's client", async () => {
    const calls: Array<[string, number, AbortSignal | undefined]> = [];
    const download: DownloadAsset = async (id, index, signal) => {
      calls.push([id, index, signal]);
      return new Blob([new Uint8Array([1, 2, 3])], { type: 'model/gltf-binary' });
    };
    const signal = new AbortController().signal;
    const res = await storedFileFetch(download, async () => { throw new Error('plain fetch must not be used'); })(ROUTE, { signal });
    expect(calls).toEqual([['gen-1', 2, signal]]);
    expect(res.ok).toBe(true);
    expect(res.headers.get('content-type')).toBe('model/gltf-binary');
    expect(new Uint8Array(await res.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]));
  });

  it("answers with the API's HTTP status when the download is refused", async () => {
    const refused = Object.assign(new Error('forbidden'), { status: 403 });
    const res = await storedFileFetch(async () => { throw refused; })(ROUTE);
    expect(res.ok).toBe(false);
    expect(res.status).toBe(403);
  });

  it('rethrows a failure that never reached the API, as fetch would', async () => {
    const offline = new TypeError('Failed to fetch');
    await expect(storedFileFetch(async () => { throw offline; })(ROUTE)).rejects.toBe(offline);
  });

  it('fetches every other URL as it is', async () => {
    const seen: unknown[] = [];
    const plain: typeof fetch = async (input, init) => {
      seen.push(input, init);
      return new Response('ok');
    };
    const init = { headers: { accept: '*/*' } };
    const res = await storedFileFetch(async () => { throw new Error('the client must not be used'); }, plain)('https://cdn.example/model.glb', init);
    expect(await res.text()).toBe('ok');
    expect(seen).toEqual(['https://cdn.example/model.glb', init]);
  });
});
