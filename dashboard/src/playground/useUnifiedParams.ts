import { useMemo, useState } from 'react';
import type { Dispatch, SetStateAction } from 'react';
import type { ModelSchema, ImageGenerationRequest } from '@litegen/sdk';
import type { MergedParam, SharedFormState } from './types';
import { mergeParams, defaultForSpec, buildRequestForModel } from './params';
import { useModelSchemas } from './useModelSchemas';

interface UnifiedParams {
  merged: MergedParam[];
  form: SharedFormState;
  /** Effective param values: user overrides merged over per-spec defaults. */
  params: Record<string, unknown>;
  setForm: Dispatch<SetStateAction<SharedFormState>>;
  setParam: (name: string, value: unknown) => void;
  schemasById: Record<string, ModelSchema>;
  buildRequests: (modelIds: string[]) => Array<{ modelId: string; request: ImageGenerationRequest }>;
}

const INITIAL: SharedFormState = { prompt: '', n: 1, strict: true, seed: '', params: {} };

export function useUnifiedParams(selectedIds: string[]): UnifiedParams {
  const schemasById = useModelSchemas(selectedIds);
  const merged = useMemo(() => mergeParams(schemasById), [schemasById]);
  const [form, setForm] = useState<SharedFormState>(INITIAL);

  // Derive effective values during render: a stored override wins, else the
  // spec default. No effect/state-sync needed — params no longer present in
  // `merged` simply fall out, and untouched params show their default.
  const params = useMemo(() => {
    const out: Record<string, unknown> = {};
    for (const p of merged) {
      out[p.name] = p.name in form.params ? form.params[p.name] : defaultForSpec(p.spec);
    }
    return out;
  }, [merged, form.params]);

  const setParam = (name: string, value: unknown) =>
    setForm(prev => ({ ...prev, params: { ...prev.params, [name]: value } }));

  const buildRequests = (modelIds: string[]) =>
    modelIds
      .filter(id => schemasById[id])
      .map(id => ({ modelId: id, request: buildRequestForModel(id, schemasById[id], { ...form, params }) }));

  return { merged, form, params, setForm, setParam, schemasById, buildRequests };
}
