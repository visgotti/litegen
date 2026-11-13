// One sample model per source format (public/samples, made by
// scripts/make-samples.mjs from litegen's own test models).
import { useEffect, useState } from 'react';
import type { Format } from '@litegen/3d-converter';

export interface Sample {
  id: string;
  title: string;
  description: string;
  format: Format;
  static: boolean;
  files: string[];
}

const base = () => new URL('./samples/', document.baseURI);

export async function loadSample(sample: Sample): Promise<Record<string, Uint8Array>> {
  const entries = await Promise.all(
    sample.files.map(async (name) => {
      const response = await fetch(new URL(`${sample.id}/${name}`, base()));
      if (!response.ok) throw new Error(`${sample.id}/${name}: HTTP ${response.status}`);
      return [name, new Uint8Array(await response.arrayBuffer())] as const;
    }),
  );
  return Object.fromEntries(entries);
}

export default function SampleGallery({ onPick, busy, current }: { onPick: (sample: Sample) => void; busy: boolean; current: string | null }) {
  const [samples, setSamples] = useState<Sample[]>([]);
  useEffect(() => {
    let cancelled = false;
    fetch(new URL('samples.json', base()))
      .then((r) => r.json() as Promise<Sample[]>)
      .then((s) => {
        if (!cancelled) setSamples(s);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  if (samples.length === 0) return null;
  return (
    <div className="samples" role="group" aria-label="Sample models">
      <span className="samples-label">Or try a sample:</span>
      {samples.map((s) => (
        <button
          key={s.id}
          type="button"
          className="sample"
          data-testid={`sample-${s.id}`}
          aria-pressed={current === s.id}
          disabled={busy}
          title={s.description}
          onClick={() => onPick(s)}
        >
          <span className="sample-format">{s.format}</span>
          <span className="sample-title">{s.title}</span>
        </button>
      ))}
    </div>
  );
}
