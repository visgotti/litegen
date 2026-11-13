import { ProviderDefinition } from '../../src/providers/types';
import { LitegenModel } from './litegen-models';

export interface Gaps {
  /** Catalog providers with no registry entry. */
  providerMissing: string[];
  /** Catalog models no seed carries. */
  modelsMissing: LitegenModel[];
  /** Seeded models the catalog no longer carries. */
  modelsRetired: string[];
}

/** Where the registry and litegen's catalog disagree, in either direction. */
export function findGaps(canonical: LitegenModel[], registry: ProviderDefinition[]): Gaps {
  const seeded = new Set(registry.flatMap((p) => p.seed.map((m) => m.id)));
  const carried = new Set(canonical.map((m) => m.id));
  const registered = new Set(registry.map((p) => p.config.id));
  return {
    providerMissing: [...new Set(canonical.map((m) => m.provider))].filter((p) => !registered.has(p)),
    modelsMissing: canonical.filter((m) => !seeded.has(m.id)),
    modelsRetired: [...seeded].filter((id) => !carried.has(id)),
  };
}
