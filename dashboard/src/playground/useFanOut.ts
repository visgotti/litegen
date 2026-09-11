import { useRef, useState } from 'react';
import { client } from '../sdk-client';
import type { ImageGenerationRequest, Model3dGenerationRequest } from '@litegen/sdk';
import type { ResultTileState } from './types';

const MAX_CONCURRENCY = 4;

/**
 * Wall-clock budget for one Playground 3D job, passed explicitly to every
 * `models3d` call. The SDK's default is 5 minutes, which a real vendor job
 * that queues and then refines can outlive: polling would abort with a
 * timeout while the server job runs on to completion and is billed. 30
 * minutes covers that without polling into the backend's 2-hour reap age
 * (MAX_ACTIVE_AGE_SECS), past which the row is failed server-side anyway.
 */
export const MODEL3D_POLL_TIMEOUT_MS = 30 * 60_000;

/**
 * Tracks which run owns the tiles.
 *
 * A rerun (Compare mode's per-tile rerun button is never disabled) starts a
 * second run over the same tile keys. Without this, the superseded run keeps
 * polling for the whole timeout and keeps patching keys it no longer owns:
 * progress alternates between two jobs, its `Promise.all` clears `running`
 * (so Cancel disappears), and after a Cancel it patches the tile back to
 * 'done'. `begin` aborts the run it replaces, and `owns` lets the loop drop
 * anything that arrives from a run that is no longer current.
 */
export function createRunEpoch() {
  let current: AbortController | null = null;
  return {
    /** Aborts the run in flight, if any, and becomes the current run. */
    begin(): AbortController {
      current?.abort();
      current = new AbortController();
      return current;
    },
    /** True while `ctrl` is still the current run's controller. */
    owns(ctrl: AbortController): boolean {
      return current === ctrl;
    },
    /** Cancels the current run but leaves it current, so it still settles the UI. */
    abort(): void {
      current?.abort();
    },
  };
}

export type RunEpoch = ReturnType<typeof createRunEpoch>;

type FanOutRequest = {
  modelId: string;
  mediaType: 'image' | 'model3d';
  request: ImageGenerationRequest | Model3dGenerationRequest;
};

interface FanOut {
  tiles: ResultTileState[];
  running: boolean;
  run: (requests: FanOutRequest[]) => Promise<void>;
  cancel: () => void;
}

export function useFanOut(): FanOut {
  const [tiles, setTiles] = useState<ResultTileState[]>([]);
  const [running, setRunning] = useState(false);
  const epochRef = useRef<RunEpoch | null>(null);
  const epoch = (epochRef.current ??= createRunEpoch());

  const setTile = (key: string, p: Partial<ResultTileState>) =>
    setTiles(prev => prev.map(t => (t.key === key ? { ...t, ...p } : t)));

  const run: FanOut['run'] = async (requests) => {
    // Supersede whatever is in flight: abort it so its polling stops, and
    // ignore anything it still reports (`patch` below is a no-op once this
    // run is no longer the current one).
    const ctrl = epoch.begin();
    setRunning(true);

    const patch = (key: string, p: Partial<ResultTileState>) => {
      if (epoch.owns(ctrl)) setTile(key, p);
    };

    // One tile per (model × n). `n` is meaningless for a mesh — the backend
    // bills a 3D job flat per generation regardless of the field (same
    // precedent as video) — so clamp to a single tile rather than seeding N
    // duplicate viewers for one job.
    const initial: ResultTileState[] = [];
    for (const { modelId, mediaType, request } of requests) {
      const n = mediaType === 'model3d' ? 1 : (request as { n?: number }).n ?? 1;
      for (let i = 0; i < n; i++) {
        initial.push({ key: `${modelId}#${i}`, modelId, index: i, status: 'queued', mediaType, request });
      }
    }
    setTiles(initial);

    // Bounded worker pool over the request list (n images come back in one call).
    let cursor = 0;
    const worker = async () => {
      while (cursor < requests.length && !ctrl.signal.aborted) {
        const { modelId, mediaType, request } = requests[cursor++];
        const keys = initial.filter(t => t.modelId === modelId).map(t => t.key);

        if (mediaType === 'model3d') {
          keys.forEach(k => patch(k, { status: 'running' }));
          const started = performance.now();
          try {
            const job = client.models3d.generate(request as Model3dGenerationRequest, {
              signal: ctrl.signal,
              intervalMs: 1500,
              timeoutMs: MODEL3D_POLL_TIMEOUT_MS,
            });
            const submitted = await job.submitted;
            keys.forEach(k => patch(k, { status: 'polling', progress: submitted.progress ?? 0 }));

            let final = submitted;
            for await (const update of client.models3d.poll(submitted.id, {
              signal: ctrl.signal,
              intervalMs: 1500,
              timeoutMs: MODEL3D_POLL_TIMEOUT_MS,
            })) {
              final = update;
              keys.forEach(k => patch(k, { progress: update.progress ?? 0 }));
            }

            const latency = Math.round(performance.now() - started);
            if (final.status !== 'completed') {
              keys.forEach(k => patch(k, {
                status: 'error',
                error: final.error ?? `job ${final.status}`,
                latencyMs: latency,
              }));
            } else {
              const mesh = final.assets?.find(a => a.kind === 'mesh');
              keys.forEach(k => patch(k, {
                status: 'done',
                latencyMs: latency,
                costUsd: final.usage?.cost_usd,
                progress: 100,
                assets: final.assets ?? [],
                // A completed 3D job always carries a mesh; its absence is a failure.
                url: mesh?.url ?? null,
                ...(mesh ? {} : { status: 'error' as const, error: 'completed without a mesh asset' }),
              }));
            }
          } catch (e) {
            if (ctrl.signal.aborted) return;
            keys.forEach(k => patch(k, { status: 'error', error: (e as Error).message }));
          }
          continue;
        }

        keys.forEach(k => patch(k, { status: 'running' }));
        const started = performance.now();
        try {
          const res = await client.images.generate(request as ImageGenerationRequest, ctrl.signal);
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
    // A superseded run must not clear `running` — the run that replaced it is
    // still going, and hiding Cancel would strand it.
    if (epoch.owns(ctrl)) setRunning(false);
  };

  const cancel = () => {
    epoch.abort();
    setRunning(false);
    // 'polling' is a live-job state introduced for 3D (queued/running never
    // reach a server job); it must be swept here too or a tile mid-poll when
    // Cancel is hit shows "generating… X%" forever, since the aborted branch
    // in the worker itself bare-returns rather than patching status.
    setTiles(prev => prev.map(t => (t.status === 'queued' || t.status === 'running' || t.status === 'polling'
      ? { ...t, status: 'error', error: 'cancelled' } : t)));
  };

  return { tiles, running, run, cancel };
}
