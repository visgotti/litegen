import { useRef, useState } from 'react';
import { client } from '../sdk-client';
import type { ImageGenerationRequest } from '@litegen/sdk';
import type { ResultTileState } from './types';

const MAX_CONCURRENCY = 4;

interface FanOut {
  tiles: ResultTileState[];
  running: boolean;
  run: (requests: Array<{ modelId: string; request: ImageGenerationRequest }>) => Promise<void>;
  cancel: () => void;
}

export function useFanOut(): FanOut {
  const [tiles, setTiles] = useState<ResultTileState[]>([]);
  const [running, setRunning] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const patch = (key: string, p: Partial<ResultTileState>) =>
    setTiles(prev => prev.map(t => (t.key === key ? { ...t, ...p } : t)));

  const run: FanOut['run'] = async (requests) => {
    const ctrl = new AbortController();
    abortRef.current = ctrl;
    setRunning(true);

    // One tile per (model × n).
    const initial: ResultTileState[] = [];
    for (const { modelId, request } of requests) {
      const n = (request as { n?: number }).n ?? 1;
      for (let i = 0; i < n; i++) {
        initial.push({ key: `${modelId}#${i}`, modelId, index: i, status: 'queued', request });
      }
    }
    setTiles(initial);

    // Bounded worker pool over the request list (n images come back in one call).
    let cursor = 0;
    const worker = async () => {
      while (cursor < requests.length && !ctrl.signal.aborted) {
        const { modelId, request } = requests[cursor++];
        const keys = initial.filter(t => t.modelId === modelId).map(t => t.key);
        keys.forEach(k => patch(k, { status: 'running' }));
        const started = performance.now();
        try {
          const res = await client.images.generate(request, ctrl.signal);
          const latency = Math.round(performance.now() - started);
          const cost = (res as { usage?: { cost_usd?: number } }).usage?.cost_usd;
          const data = (res as { data?: Array<{ b64_json?: string | null; url?: string | null }> }).data ?? [];
          keys.forEach((k, i) => patch(k, {
            status: 'done', latencyMs: latency, costUsd: cost,
            b64_json: data[i]?.b64_json ?? null, url: data[i]?.url ?? null,
          }));
        } catch (e) {
          if (ctrl.signal.aborted) return;
          const msg = (e as Error).message;
          keys.forEach(k => patch(k, { status: 'error', error: msg }));
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(MAX_CONCURRENCY, requests.length) }, worker));
    setRunning(false);
  };

  const cancel = () => {
    abortRef.current?.abort();
    setRunning(false);
    setTiles(prev => prev.map(t => (t.status === 'queued' || t.status === 'running'
      ? { ...t, status: 'error', error: 'cancelled' } : t)));
  };

  return { tiles, running, run, cancel };
}
