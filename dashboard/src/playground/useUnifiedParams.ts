import { useMemo, useState } from 'react';
import type { Dispatch, SetStateAction } from 'react';
import type { ModelSchema, ImageGenerationRequest } from '@litegen/sdk';
import type { MergedExtra, MergedParam, MergedRole, SharedFormState } from './types';
import {
  buildRequestForModel,
  defaultForSpec,
  maxTotalRefs,
  mergeExtras,
  mergeParams,
  mergeRefRoles,
  requestProblems,
} from './params';
import { useModelSchemas } from './useModelSchemas';

interface UnifiedParams {
  merged: MergedParam[];
  /** Reference-image roles the selected models accept. */
  roles: MergedRole[];
  /** The tightest `max_total` among the selected models, if any declares one. */
  maxRefs: number | undefined;
  /** Provider-passthrough keys the selected models allow. */
  extras: MergedExtra[];
  form: SharedFormState;
  /** What each control shows: the user's value, else the schema's default. */
  params: Record<string, unknown>;
  setForm: Dispatch<SetStateAction<SharedFormState>>;
  setParam: (name: string, value: unknown) => void;
  /** Forget the user's value so the model's own default applies again. */
  resetParam: (name: string) => void;
  schemasById: Record<string, ModelSchema>;
  /** Per selected model, why it cannot run yet (empty array when it can). */
  problems: (modelIds: string[]) => Array<{ modelId: string; problems: string[] }>;
  buildRequests: (
    modelIds: string[],
    mediaTypeOf?: (modelId: string) => 'image' | 'model3d',
  ) => Array<{ modelId: string; request: ImageGenerationRequest }>;
}

const INITIAL: SharedFormState = { prompt: '', n: 1, strict: true, seed: '', params: {}, refs: [], extra: {} };

/** `skip` names params the caller renders a dedicated control for. */
export function useUnifiedParams(selectedIds: string[], skip?: ReadonlySet<string>): UnifiedParams {
  const schemasById = useModelSchemas(selectedIds);
  const merged = useMemo(() => mergeParams(schemasById, skip), [schemasById, skip]);
  const roles = useMemo(() => mergeRefRoles(schemasById), [schemasById]);
  const maxRefs = useMemo(() => maxTotalRefs(schemasById), [schemasById]);
  const extras = useMemo(() => mergeExtras(schemasById), [schemasById]);
  const [form, setForm] = useState<SharedFormState>(INITIAL);

  // Derive display values during render: a stored override wins, else the
  // spec default. No effect/state-sync needed — params no longer present in
  // `merged` simply fall out, and untouched params show their default. Only
  // `form.params` (what the user actually set) is ever sent.
  const params = useMemo(() => {
    const out: Record<string, unknown> = {};
    for (const p of merged) {
      out[p.name] = p.name in form.params ? form.params[p.name] : defaultForSpec(p.spec);
    }
    return out;
  }, [merged, form.params]);

  const setParam = (name: string, value: unknown) =>
    setForm(prev => ({ ...prev, params: { ...prev.params, [name]: value } }));

  const resetParam = (name: string) =>
    setForm(prev => {
      const rest = { ...prev.params };
      delete rest[name];
      return { ...prev, params: rest };
    });

  const problems = (modelIds: string[]) =>
    modelIds.map(id => ({ modelId: id, problems: schemasById[id] ? requestProblems(schemasById[id], form) : [] }));

  const buildRequests: UnifiedParams['buildRequests'] = (modelIds, mediaTypeOf) =>
    modelIds
      .filter(id => schemasById[id])
      .map(id => ({
        modelId: id,
        request: buildRequestForModel(id, schemasById[id], form, mediaTypeOf?.(id) ?? 'image'),
      }));

  return { merged, roles, maxRefs, extras, form, params, setForm, setParam, resetParam, schemasById, problems, buildRequests };
}
