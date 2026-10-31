import { useCallback, useEffect, useState } from 'react';
import type { Generation } from '@litegen/sdk';
import { client } from '../sdk-client';

/** How many recent generations the source picker looks through. */
const RECENT = 100;

/**
 * The caller's completed generations of one media type, newest first: what a
 * `source_model` / `source_video` picker offers (an operation reads a
 * generation by id, `GET /v1/generations/{id}`). `enabled` false skips the
 * fetch while the control is set to a URL.
 */
export function useRecentGenerations(mediaType: 'model3d' | 'video', enabled: boolean) {
  const [tick, setTick] = useState(0);
  // Which request the stored list answers: loading is "the current request
  // has not answered yet", derived rather than set from inside the effect.
  const [result, setResult] = useState<{ key: string; items: Generation[] }>({ key: '', items: [] });
  const key = `${mediaType}:${tick}`;

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    client.generations.list({ per_page: RECENT })
      .then(page => {
        if (!cancelled) setResult({ key, items: page.data.filter(g => g.media_type === mediaType && g.status === 'completed') });
      })
      .catch(() => { if (!cancelled) setResult({ key, items: [] }); });
    return () => { cancelled = true; };
  }, [key, mediaType, enabled]);

  const reload = useCallback(() => setTick(t => t + 1), []);
  return { items: result.items, loading: enabled && result.key !== key, reload };
}
