import * as fs from 'fs';
import * as path from 'path';
import { parse } from 'yaml';

/**
 * Reads the canonical model definitions from litegen-core's `models/*.yaml`.
 * Used ONLY by dev/CI tooling (the coverage check) — there is no runtime
 * coupling to litegen-core. The `mock` provider is excluded as test-only.
 */
export interface LitegenModel {
  id: string;
  provider: string;
  mediaType: 'image' | 'video' | string;
  baseCostUsd: number | null;
}

interface RawYaml {
  models?: Array<{
    id: string;
    provider: string;
    media_type?: string;
    pricing?: { base_cost_usd?: number };
  }>;
}

export function readLitegenModels(modelsDir: string): LitegenModel[] {
  const dir = path.resolve(process.cwd(), modelsDir);
  if (!fs.existsSync(dir)) {
    throw new Error(
      `litegen models dir not found: ${dir} (set LITEGEN_MODELS_DIR). ` +
        'This path is read only for the coverage check.',
    );
  }
  const out: LitegenModel[] = [];
  for (const file of fs.readdirSync(dir)) {
    if (!file.endsWith('.yaml') && !file.endsWith('.yml')) {
      continue;
    }
    if (file.startsWith('mock')) {
      continue; // test-only provider
    }
    const parsed = parse(fs.readFileSync(path.join(dir, file), 'utf8')) as RawYaml;
    for (const m of parsed.models ?? []) {
      if (m.provider === 'mock') {
        continue;
      }
      out.push({
        id: m.id,
        provider: m.provider,
        mediaType: m.media_type ?? 'image',
        baseCostUsd: m.pricing?.base_cost_usd ?? null,
      });
    }
  }
  return out;
}
