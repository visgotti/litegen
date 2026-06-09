import { useEffect, useRef, useState } from 'react';
import { client } from '../sdk-client';
import type { ModelSchema } from '@litegen/sdk';

/** Fetch + session-cache ModelSchema for a set of model ids. */
export function useModelSchemas(ids: string[]): Record<string, ModelSchema> {
  const cache = useRef<Map<string, ModelSchema>>(new Map());
  const [schemas, setSchemas] = useState<Record<string, ModelSchema>>({});

  useEffect(() => {
    let cancelled = false;
    const missing = ids.filter(id => !cache.current.has(id));
    Promise.all(missing.map(id =>
      client.models.getSchema(id)
        .then(s => { cache.current.set(id, s as ModelSchema); })
        .catch(() => { /* skip unresolved schema */ }),
    )).then(() => {
      if (cancelled) return;
      const next: Record<string, ModelSchema> = {};
      for (const id of ids) {
        const s = cache.current.get(id);
        if (s) next[id] = s;
      }
      setSchemas(next);
    });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ids.join(',')]);

  return schemas;
}
