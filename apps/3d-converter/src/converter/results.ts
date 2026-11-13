// What the worker sends back about a model it read. Measuring (`info`) can
// fail where reading and writing succeed: the converter measures by placing
// every instance of every mesh, and a model past its merged-mesh cap still
// converts. So a read always succeeds here, with the measurements or the
// reason there are none.
import type { Model, ModelInfo } from '@litegen/3d-converter';
import type { ReadResult } from './protocol';

export function measured(model: Pick<Model, 'info'>): { info: ModelInfo } | { infoError: string } {
  try {
    return { info: model.info };
  } catch (e) {
    return { infoError: e instanceof Error ? e.message : String(e) };
  }
}

/** The files the package itself recognises as a model (its detectFormat),
 *  by name: what the page offers when several could be the model. */
export function modelCandidates(files: Record<string, Uint8Array>, detect: (data: Uint8Array, fileName?: string) => unknown): string[] {
  return Object.keys(files)
    .filter((name) => detect(files[name]!, name) !== undefined)
    .sort();
}

export function readResult(model: Pick<Model, 'format' | 'info' | 'warnings' | 'files'>): ReadResult {
  return { format: model.format, ...measured(model), warnings: model.warnings, files: model.files };
}
