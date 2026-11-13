// The converter, off the page's thread: @litegen/3d-converter in a module
// worker. Dispatch only; every rule is the package's.
import type { Model, ModelInfo } from '@litegen/3d-converter';
import type { Reply, Request, WireError } from './protocol';
import { measured, modelCandidates, readResult } from './results';

// The package instantiates its wasm with a top-level await. Imported
// statically, it would hold this module's evaluation until then, and a
// message the page posts meanwhile (its first `formats`) would arrive before
// `onmessage` is set and be lost. So the handler is set first, and each
// message waits for the package.
const converter = import('@litegen/3d-converter');

let model: Model | null = null;

function wireError(e: unknown): WireError {
  const err = e as { name?: string; code?: string; message?: string; param?: string; candidates?: string[] };
  // A wasm trap leaves the instance unusable: report it as a crash, and stop,
  // so the page starts a fresh worker.
  if (e instanceof WebAssembly.RuntimeError) return { name: 'RuntimeError', code: 'worker_crashed', message: String(err.message) };
  return {
    name: err.name ?? 'Error',
    code: err.code ?? 'worker_crashed',
    message: err.message ?? String(e),
    ...(err.param !== undefined ? { param: err.param } : {}),
    ...(err.candidates ? { candidates: err.candidates } : {}),
  };
}

self.onmessage = async (e: MessageEvent<Request>) => {
  const request = e.data;
  const reply = (r: Reply, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(r, transfer);
  try {
    const { formats, readModel, detectFormat } = await converter;
    switch (request.op) {
      case 'formats':
        return reply({ id: request.id, ok: true, value: formats() });
      case 'read': {
        model?.free();
        model = null;
        try {
          model = readModel(request.files, request.options);
        } catch (e) {
          // Several files could be the model: say which, so the page can ask.
          if ((e as { code?: string }).code === 'input_ambiguous') Object.assign(e as object, { candidates: modelCandidates(request.files, detectFormat) });
          throw e;
        }
        return reply({ id: request.id, ok: true, value: readResult(model) });
      }
      case 'write': {
        if (!model) throw { name: 'Error', code: 'format_unrecognized', message: 'no model has been read yet' };
        const started = performance.now();
        const conversion = model.write(request.options);
        const ms = performance.now() - started;
        let info: ModelInfo | undefined;
        // A zip is shown through the same model's `files` write, which is
        // measured; reading the zip back would measure it a second time.
        if (request.options.package !== 'zip') try {
          const back = readModel(Object.fromEntries(conversion.files.map((f) => [f.name, f.data])));
          try {
            const m = measured(back);
            info = 'info' in m ? m.info : undefined;
          } finally {
            back.free();
          }
        } catch (e) {
          // A trap leaves the instance unusable: report the crash (below),
          // so the page starts a fresh worker, instead of serving from it.
          if (e instanceof WebAssembly.RuntimeError) throw e;
          info = undefined;
        }
        // The written files move to the page without a copy.
        return reply({ id: request.id, ok: true, value: { conversion, ms, info } }, conversion.files.map((f) => f.data.buffer));
      }
    }
  } catch (err) {
    const error = wireError(err);
    reply({ id: request.id, ok: false, error });
    if (error.code === 'worker_crashed') self.close();
  }
};
