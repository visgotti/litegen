import React, { useEffect, useState } from 'react';
import type { Generation } from '@litegen/sdk';

/** One entry of a 3D generation's re-hosted asset list (metadata.assets). */
interface Asset {
  kind: 'mesh' | 'texture' | 'preview';
  url: string;
  format: string;
}

function assets(g: Generation): Asset[] {
  const raw = (g.metadata as { assets?: unknown } | null | undefined)?.assets;
  return Array.isArray(raw) ? (raw as Asset[]) : [];
}

/** Primary mesh URL. Falls back to `result_url`, which the API also sets to the
 *  mesh, so a row written before assets[] existed still renders.
 *  Co-located with the component (not split into a separate module) because
 *  callers need both the URL helpers and the viewer together; that mixes a
 *  non-component export into this file, which react-refresh flags. */
// eslint-disable-next-line react-refresh/only-export-components
export function meshUrl(g: Generation): string | null {
  return assets(g).find(a => a.kind === 'mesh')?.url ?? g.result_url ?? null;
}

/** 2D preview for thumbnail/grid contexts, where a live GL canvas per row would
 *  be wasteful. Null when the provider returned no preview. */
// eslint-disable-next-line react-refresh/only-export-components
export function previewUrl(g: Generation): string | null {
  return assets(g).find(a => a.kind === 'preview')?.url ?? null;
}

/**
 * Lazily loads the `<model-viewer>` custom element the first time a 3D result is
 * actually shown, so the ~1MB WebGL bundle never lands on a dashboard session
 * that only looks at images.
 */
export default function ModelViewer({
  src,
  poster,
  testId,
  style,
}: {
  src: string;
  poster?: string | null;
  testId?: string;
  style?: React.CSSProperties;
}) {
  const [ready, setReady] = useState(() => !!customElements.get('model-viewer'));
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (ready) return;
    let cancelled = false;
    import('@google/model-viewer')
      .then(() => { if (!cancelled) setReady(true); })
      .catch(() => { if (!cancelled) setFailed(true); });
    return () => { cancelled = true; };
  }, [ready]);

  if (failed) {
    // A GL/bundle failure must not swallow the result — the mesh is still
    // downloadable, so degrade to a link rather than an empty box.
    return (
      <a href={src} data-testid={testId ? `${testId}-fallback` : undefined} style={{ color: '#58a6ff' }}>
        Download mesh
      </a>
    );
  }
  if (!ready) {
    return <span data-testid={testId ? `${testId}-loading` : undefined} style={{ color: '#8b949e' }}>loading viewer…</span>;
  }

  return React.createElement('model-viewer', {
    src,
    poster: poster ?? undefined,
    'camera-controls': true,
    'auto-rotate': true,
    'shadow-intensity': '1',
    'data-testid': testId,
    style: { width: '100%', height: 360, background: '#0d1117', borderRadius: 6, ...style },
  });
}
