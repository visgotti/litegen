// MeshViewer for a model at a URL: downloads it with progress, then draws it.
// A download that fails offers Retry; a file whose loader can't read it does
// not, since a second download brings the same bytes. A format the viewer
// has no loader for, or a browser without WebGL 2, is never downloaded for.
import { useEffect, useState } from 'react';
import { isViewerFormat, type ViewerFile } from '../types';
import { startDownload } from './fetch-model';
import MeshViewer, { type MeshViewerProps } from './MeshViewer';
import { hasWebGL2 } from './webgl';

export interface RemoteMeshViewerProps extends Omit<MeshViewerProps, 'files' | 'progress' | 'failure' | 'onRetry' | 'downloadUrl'> {
  /** The model file. Its own references (an OBJ's `.mtl`, a glTF's `.bin`)
   *  are not followed: a remote model is shown as the one file. */
  url: string;
  /** Fetches the file; the browser's `fetch` by default. */
  fetch?: typeof globalThis.fetch;
}

interface Download {
  /** The download this belongs to: the URL and the attempt. */
  key: string;
  progress: number | null;
  /** Made once per finished download, so the viewer loads each once. */
  files?: ViewerFile[];
  failure?: string;
}

const nameOf = (url: string, format: string) => {
  const path = url.split(/[?#]/, 1)[0] ?? '';
  const name = path.slice(path.lastIndexOf('/') + 1);
  return /\.[a-z0-9]+$/i.test(name) ? name : `model.${format}`;
};

export default function RemoteMeshViewer({ url, format, fetch: fetcher, ...viewer }: RemoteMeshViewerProps) {
  const [attempt, setAttempt] = useState(0);
  const key = `${attempt}\n${url}`;
  // Nothing is known of the progress until something arrives: the bar
  // sweeps rather than claim 0% (a file the dashboard's file route serves
  // arrives all at once, so it never reports a share).
  const [download, setDownload] = useState<Download>({ key, progress: null });
  // Keyed by URL and attempt and derived during render, so a new model never
  // shows the last one's progress or failure.
  const current: Download = download.key === key ? download : { key, progress: null };
  const drawable = isViewerFormat(format);

  useEffect(() => {
    if (!drawable || !hasWebGL2()) return;
    let shown = -1;
    // Cancelled on a new URL, a Retry or unmount: a superseded download
    // reports nothing, so it never writes over the one that replaced it.
    return startDownload(url, {
      fetch: fetcher,
      onProgress: (p) => {
        // One update per whole percent, not one per network chunk.
        const pct = p == null ? -2 : Math.floor(p * 100);
        if (pct === shown) return;
        shown = pct;
        setDownload({ key, progress: p });
      },
      onDone: (data) => setDownload({ key, progress: 1, files: [{ name: nameOf(url, format), data, role: 'model' }] }),
      onFail: (e) => setDownload({ key, progress: null, failure: e.message }),
    });
  }, [key, url, format, drawable, fetcher]);

  return (
    <MeshViewer
      {...viewer}
      format={format}
      files={current.files ?? null}
      progress={current.progress}
      failure={current.failure ?? null}
      onRetry={() => setAttempt((n) => n + 1)}
      downloadUrl={url}
    />
  );
}
