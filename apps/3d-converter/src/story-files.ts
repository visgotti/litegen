// Model files for stories, fetched from Storybook's static folders
// (.storybook/main.ts): mesh's test vectors, the dashboard's fixtures and
// this app's samples.
import type { ViewerFile } from '@litegen/3d-viewer';

export async function fetchFiles(paths: string[], main?: string): Promise<ViewerFile[]> {
  return Promise.all(
    paths.map(async (path) => {
      const response = await fetch(path);
      if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
      const name = path.split('/').pop()!;
      return { name, data: new Uint8Array(await response.arrayBuffer()), role: name === (main ?? paths[0]!.split('/').pop()) ? 'model' : undefined };
    }),
  );
}

export const text = (name: string, body: string): ViewerFile => ({ name, data: new TextEncoder().encode(body), role: 'model' });
