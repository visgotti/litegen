// `fetch` for a stored file, for the shared 3D viewer, which downloads its
// model itself: litegen's file route (`fileRouteOf`) goes through the
// dashboard's own client, with its credentials and active tenant, as
// `useFileSrc` does for an <img>. Every other URL is fetched as it is.
import { fileRouteOf } from './file-route';

/** The client's file download: `client.generations.downloadAsset`. */
export type DownloadAsset = (generationId: string, index: number, signal?: AbortSignal) => Promise<Blob>;

const urlOf = (input: RequestInfo | URL) => (typeof input === 'string' ? input : input instanceof URL ? input.href : input.url);

export function storedFileFetch(download: DownloadAsset, plain: typeof fetch = (input, init) => fetch(input, init)): typeof fetch {
  return async (input, init) => {
    const route = fileRouteOf(urlOf(input));
    if (!route) return plain(input, init);
    let blob: Blob;
    try {
      blob = await download(route.generationId, route.index, init?.signal ?? undefined);
    } catch (e) {
      // The API answered with an error (the client's LiteGenAPIError): the
      // same answer fetch would give, so the viewer reports its status.
      const status = (e as { status?: unknown } | null)?.status;
      if (typeof status === 'number' && status >= 400 && status <= 599) return new Response(null, { status });
      throw e;
    }
    return new Response(blob, { headers: blob.type ? { 'content-type': blob.type } : {} });
  };
}
