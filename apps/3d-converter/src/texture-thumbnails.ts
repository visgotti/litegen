// A model's texture files as thumbnails, as the dashboard shows a delivered
// mesh's textures: blob: URLs made for each files array and released with it.
import { useEffect, useState } from 'react';
import type { Thumbnail, ViewerFile } from '@litegen/3d-viewer';

/** What a browser draws in an <img>, by extension, as the media type its
 *  blob gets: a KTX2 or EXR texture is downloadable, not previewable. */
const BROWSER_IMAGE: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  gif: 'image/gif',
  avif: 'image/avif',
};

function imageType(name: string): string | undefined {
  const dot = name.lastIndexOf('.');
  return dot < 0 ? undefined : BROWSER_IMAGE[name.slice(dot + 1).toLowerCase()];
}

export function textureFilesOf(files: ViewerFile[]): ViewerFile[] {
  return files.filter((f) => f.role === 'texture' && imageType(f.name) !== undefined);
}

/** A texture's bytes as a blob a thumbnail can show and link to. The bytes
 *  come from a dropped model, so the blob is typed as the image its name
 *  says: a thumbnail opened in a tab is then shown as that image, or not at
 *  all, and never sniffed into a page on this origin. */
export function thumbnailBlob(file: ViewerFile): Blob {
  return new Blob([file.data as Uint8Array<ArrayBuffer>], { type: imageType(file.name) ?? 'application/octet-stream' });
}

export function useTextureThumbnails(files: ViewerFile[]): Thumbnail[] {
  const [made, setMade] = useState<{ files: ViewerFile[]; items: Thumbnail[] } | null>(null);
  useEffect(() => {
    const items = textureFilesOf(files).map((f) => {
      const name = f.name.slice(f.name.lastIndexOf('/') + 1);
      return { url: URL.createObjectURL(thumbnailBlob(f)), label: name, alt: `Texture ${name}` };
    });
    setMade({ files, items });
    return () => items.forEach((t) => URL.revokeObjectURL(t.url));
  }, [files]);
  return made?.files === files ? made.items : [];
}
