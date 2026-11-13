// litegen's file route, `…/v1/generations/{id}/assets/{index}`: the URL an app
// in `private` mode hands out for every stored file (docs/storage-access.md).
// It wants the caller's credentials, which an <img> or <video> cannot send when
// they are an API key; see `useFileSrc`, and `fetchStoredFile` for the 3D viewer.
const FILE_ROUTE = /^https?:\/\/[^/?#]+(?:\/[^?#]*)?\/v1\/generations\/([^/?#]+)\/assets\/(\d+)(?:#.*)?$/;

/** The generation and index a file-route URL names, or `null` for any other URL. */
export function fileRouteOf(url: string): { generationId: string; index: number } | null {
  const m = FILE_ROUTE.exec(url);
  return m ? { generationId: decodeURIComponent(m[1]), index: Number(m[2]) } : null;
}

/** Whether `url` is litegen's file route. */
export function isFileRoute(url: string): boolean {
  return fileRouteOf(url) !== null;
}
