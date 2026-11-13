import { describe, expect, it } from 'vitest';
import { fetchModel, ModelFetchError, startDownload } from './fetch-model';

const bytes = (n: number, fill: number) => new Uint8Array(n).fill(fill);

/** A body that arrives in `chunks`, as a network response does. */
function streamOf(chunks: Uint8Array[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (const c of chunks) controller.enqueue(c);
      controller.close();
    },
  });
}

const serving = (response: () => Response): typeof fetch => async () => response();

describe('fetchModel', () => {
  it('returns the whole file, reporting progress that only grows and ends at 1', async () => {
    const chunks = [bytes(10, 1), bytes(20, 2), bytes(30, 3)];
    const progress: Array<number | null> = [];
    const data = await fetchModel('https://x/model.glb', {
      fetch: serving(() => new Response(streamOf(chunks), { headers: { 'content-length': '60' } })),
      onProgress: (p) => progress.push(p),
    });
    expect(data).toEqual(new Uint8Array([...chunks[0]!, ...chunks[1]!, ...chunks[2]!]));
    expect(progress.length).toBeGreaterThanOrEqual(2);
    expect(progress[0]).toBeCloseTo(10 / 60);
    for (let i = 1; i < progress.length; i++) expect(progress[i]!).toBeGreaterThanOrEqual(progress[i - 1]!);
    expect(progress.at(-1)).toBe(1);
  });

  it('reports unknown progress while the size is unknown, then 1', async () => {
    const progress: Array<number | null> = [];
    await fetchModel('https://x/model.glb', {
      fetch: serving(() => new Response(streamOf([bytes(5, 1), bytes(5, 2)]))),
      onProgress: (p) => progress.push(p),
    });
    expect(progress.slice(0, -1).every((p) => p === null)).toBe(true);
    expect(progress.at(-1)).toBe(1);
  });

  it('treats a compressed transfer as of unknown size: its length is not the bytes read', async () => {
    const progress: Array<number | null> = [];
    await fetchModel('https://x/model.glb', {
      fetch: serving(() => new Response(streamOf([bytes(50, 1), bytes(50, 2)]), { headers: { 'content-length': '20', 'content-encoding': 'gzip' } })),
      onProgress: (p) => progress.push(p),
    });
    expect(progress.slice(0, -1).every((p) => p === null)).toBe(true);
    expect(progress.at(-1)).toBe(1);
  });

  it('fails with the HTTP status when the server answers with an error', async () => {
    for (const status of [403, 404, 500]) {
      const failure = await fetchModel('https://x/model.glb', { fetch: serving(() => new Response('no', { status })) }).catch((e: unknown) => e);
      expect(failure).toBeInstanceOf(ModelFetchError);
      expect((failure as ModelFetchError).status).toBe(status);
      expect((failure as ModelFetchError).message).toBe(`The model file could not be downloaded (HTTP ${status}).`);
    }
  });

  it('fails with no status when no response arrives', async () => {
    const failure = await fetchModel('https://x/model.glb', {
      fetch: async () => {
        throw new TypeError('Failed to fetch');
      },
    }).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ModelFetchError);
    expect((failure as ModelFetchError).status).toBeNull();
    expect((failure as ModelFetchError).message).toBe('The model file could not be downloaded: Failed to fetch');
  });

  it('fails as a download failure when the body breaks off part-way', async () => {
    const broken = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes(10, 1));
        controller.error(new TypeError('network error'));
      },
    });
    const failure = await fetchModel('https://x/model.glb', { fetch: serving(() => new Response(broken)) }).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ModelFetchError);
    expect((failure as ModelFetchError).message).toBe('The model file stopped downloading: network error');
  });

  it('rejects with the abort itself when cancelled, not a download failure', async () => {
    const controller = new AbortController();
    const pending = fetchModel('https://x/model.glb', {
      signal: controller.signal,
      fetch: (_url, init) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener('abort', () => reject(new DOMException('The operation was aborted.', 'AbortError')));
        }),
    });
    controller.abort();
    const failure = await pending.catch((e: unknown) => e);
    expect(failure).not.toBeInstanceOf(ModelFetchError);
    expect((failure as DOMException).name).toBe('AbortError');
  });
});

describe('startDownload', () => {
  const settle = () => new Promise((r) => setTimeout(r, 20));

  it('reports progress, then the file, once', async () => {
    const calls: string[] = [];
    startDownload('https://x/model.glb', {
      fetch: serving(() => new Response(streamOf([bytes(4, 1), bytes(4, 2)]), { headers: { 'content-length': '8' } })),
      onProgress: (p) => calls.push(`progress ${p}`),
      onDone: (data) => calls.push(`done ${data.length}`),
      onFail: (e) => calls.push(`fail ${e.message}`),
    });
    await settle();
    expect(calls).toEqual(['progress 0.5', 'progress 1', 'progress 1', 'done 8']);
  });

  it('reports a failed download, once', async () => {
    const calls: string[] = [];
    startDownload('https://x/model.glb', {
      fetch: serving(() => new Response('no', { status: 404 })),
      onProgress: () => calls.push('progress'),
      onDone: () => calls.push('done'),
      onFail: (e) => calls.push(`fail ${e.message}`),
    });
    await settle();
    expect(calls).toEqual(['fail The model file could not be downloaded (HTTP 404).']);
  });

  it('says nothing once cancelled, even when the file arrives afterwards', async () => {
    // A fetch the abort cannot stop: the dashboard's file route resolves
    // with a finished blob, which no signal can take back.
    let answer!: (r: Response) => void;
    const calls: string[] = [];
    const cancel = startDownload('https://x/model.glb', {
      fetch: () => new Promise<Response>((resolve) => { answer = resolve; }),
      onProgress: () => calls.push('progress'),
      onDone: () => calls.push('done'),
      onFail: () => calls.push('fail'),
    });
    cancel();
    answer(new Response(new Uint8Array([1, 2, 3])));
    await settle();
    expect(calls).toEqual([]);
  });
});
