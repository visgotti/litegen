import { useEffect, useState } from 'react';
import { client } from './sdk-client';
import { fileRouteOf } from './file-route';
import { storedFileFetch } from './file-fetch';

type Fetched = { url: string; src: string; mime?: string };

/**
 * The `src` to give an element showing a stored file. Every URL is used as it
 * is, except litegen's file route (`fileRouteOf`): that file is downloaded
 * through the dashboard's own client — its credentials and active tenant —
 * and handed over as a `blob:` URL. `undefined` while it loads; if the
 * download fails, the URL itself, so the element shows its own error.
 */
export function useFileSrc(url: string | null | undefined): string | undefined {
  return useFileSource(url).src;
}

/**
 * [`useFileSrc`], with the content type the file route served the file as
 * (`undefined` for any other URL, or while it loads): a file-route URL has no
 * extension to tell a GIF from a video by.
 */
export function useFileSource(url: string | null | undefined): { src: string | undefined; mime: string | undefined } {
  const [fetched, setFetched] = useState<Fetched | null>(null);
  useEffect(() => {
    const route = url ? fileRouteOf(url) : null;
    if (!url || !route) return;
    const controller = new AbortController();
    let objectUrl: string | null = null;
    client.generations
      .downloadAsset(route.generationId, route.index, controller.signal)
      .then(blob => {
        objectUrl = URL.createObjectURL(blob);
        setFetched({ url, src: objectUrl, mime: blob.type || undefined });
      })
      .catch(() => {
        if (!controller.signal.aborted) setFetched({ url, src: url });
      });
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [url]);
  if (!url) return { src: undefined, mime: undefined };
  if (!fileRouteOf(url)) return { src: url, mime: undefined };
  return fetched?.url === url ? { src: fetched.src, mime: fetched.mime } : { src: undefined, mime: undefined };
}

/** `fetch` for a stored file, for the 3D viewer, which downloads its model
 *  itself: the file route goes through the dashboard's client, as above. */
export const fetchStoredFile = storedFileFetch((id, index, signal) => client.generations.downloadAsset(id, index, signal));
