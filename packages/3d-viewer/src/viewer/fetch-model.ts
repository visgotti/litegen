// Downloads a model file for the viewer, with progress, and says why it
// failed. Fetching it here, rather than handing three.js's loaders a URL,
// means the viewer knows the HTTP status of its own request: a download that
// failed (Retry can help) is told apart from a file that arrived and doesn't
// parse (Retry would fetch the same bytes), which <model-viewer> hid
// (dashboard/src/components/model3d-assets.ts, classifyViewerError, before
// this package).

/** A download that failed: no response, an error status, or a body that
 *  broke off. Retrying can help. */
export class ModelFetchError extends Error {
  /** The HTTP status, or null when no response arrived. */
  readonly status: number | null;

  constructor(message: string, status: number | null) {
    super(message);
    this.name = 'ModelFetchError';
    this.status = status;
  }
}

export interface FetchModelOptions {
  fetch?: typeof globalThis.fetch;
  signal?: AbortSignal;
  /** The share of the file received, 0 to 1, or null while its size is
   *  unknown. The last call is always 1. */
  onProgress?: (fraction: number | null) => void;
}

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The size the body will have once read, when the response says it: a
 *  compressed transfer's `content-length` counts compressed bytes, not the
 *  bytes the stream gives. */
function sizeOf(response: Response): number | null {
  const encoding = response.headers.get('content-encoding');
  if (encoding && encoding !== 'identity') return null;
  const length = Number(response.headers.get('content-length'));
  return Number.isFinite(length) && length > 0 ? length : null;
}

/** The file at `url`. Rejects with a ModelFetchError for a failed download,
 *  or with the abort itself when `signal` cancels it. */
export async function fetchModel(url: string, options: FetchModelOptions = {}): Promise<Uint8Array> {
  const { signal, onProgress } = options;
  const get = options.fetch ?? globalThis.fetch;
  let response: Response;
  try {
    response = await get(url, { signal });
  } catch (e) {
    if (signal?.aborted) throw e;
    throw new ModelFetchError(`The model file could not be downloaded: ${messageOf(e)}`, null);
  }
  if (!response.ok) throw new ModelFetchError(`The model file could not be downloaded (HTTP ${response.status}).`, response.status);
  if (!response.body) {
    const data = new Uint8Array(await response.arrayBuffer());
    onProgress?.(1);
    return data;
  }
  const size = sizeOf(response);
  const chunks: Uint8Array[] = [];
  let received = 0;
  const reader = response.body.getReader();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      received += value.length;
      onProgress?.(size ? Math.min(received / size, 1) : null);
    }
  } catch (e) {
    if (signal?.aborted) throw e;
    throw new ModelFetchError(`The model file stopped downloading: ${messageOf(e)}`, response.status);
  }
  const data = new Uint8Array(received);
  let at = 0;
  for (const c of chunks) {
    data.set(c, at);
    at += c.length;
  }
  onProgress?.(1);
  return data;
}

export interface DownloadHandlers {
  fetch?: typeof globalThis.fetch;
  onProgress?: (fraction: number | null) => void;
  onDone: (data: Uint8Array) => void;
  onFail: (error: Error) => void;
}

/** Starts downloading `url`, reporting through the handlers; the function it
 *  returns cancels it. Nothing is reported after a cancel, even from a fetch
 *  the abort cannot stop (one that resolves with a finished blob, as the
 *  dashboard's file route does), so a superseded download never writes over
 *  the one that replaced it. */
export function startDownload(url: string, { fetch, onProgress, onDone, onFail }: DownloadHandlers): () => void {
  const controller = new AbortController();
  const live = () => !controller.signal.aborted;
  fetchModel(url, { fetch, signal: controller.signal, onProgress: (p) => live() && onProgress?.(p) }).then(
    (data) => live() && onDone(data),
    (e: unknown) => live() && onFail(e instanceof Error ? e : new Error(String(e))),
  );
  return () => controller.abort();
}
