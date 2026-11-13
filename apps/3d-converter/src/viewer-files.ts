// A zip's entries as the shared viewer takes them. The dashboard never opens
// a zip, so this stays in the tool rather than in @litegen/3d-viewer.
import type { FileRole } from '@litegen/3d-converter';
import type { ViewerFile } from '@litegen/3d-viewer';

const normalize = (name: string) => name.replace(/\\/g, '/').replace(/^(\.\/|\/)+/, '');

/** Every entry but a Mac's own (`__MACOSX/`) and folders, named with forward
 *  slashes as the read's file list names them, so each keeps the role the
 *  converter gave it. */
export function viewerFilesOf(entries: Record<string, Uint8Array>, roles: Map<string, FileRole>): ViewerFile[] {
  return Object.entries(entries)
    .map(([name, data]) => ({ name: normalize(name), data }))
    .filter(({ name }) => !name.endsWith('/') && !name.startsWith('__MACOSX/'))
    .map(({ name, data }) => {
      const role = roles.get(name);
      return role ? { name, data, role } : { name, data };
    });
}
