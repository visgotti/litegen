// The page's side of the converter worker: typed calls, matched to replies
// by id. A worker that dies (a wasm trap, out of memory) fails its pending
// calls with `worker_crashed`; the next call starts a new worker and replays
// the last read, so the model is there again.
import type { ErrorCode, FormatInfo, ReadOptions, WriteOptions } from '@litegen/3d-converter';
import type { ReadResult, Reply, Request, WriteResult } from './protocol';

export interface ClientError {
  code: ErrorCode | 'param_required' | 'worker_crashed';
  message: string;
  param?: string;
  /** With `input_ambiguous`: the files that could be the model. */
  candidates?: string[];
}

type Pending = { resolve: (value: unknown) => void; reject: (error: ClientError) => void };
/** A request before it has an id; Omit distributed over the union. */
type Call = Request extends infer R ? (R extends Request ? Omit<R, 'id'> : never) : never;

const defaultWorker = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

export class ConverterClient {
  private worker: Worker | null = null;
  private next = 1;
  private readonly pending = new Map<number, Pending>();
  private lastRead: Call | null = null;
  private readonly createWorker: () => Worker;

  constructor(createWorker: () => Worker = defaultWorker) {
    this.createWorker = createWorker;
  }

  formats(): Promise<FormatInfo[]> {
    return this.call({ op: 'formats' }) as Promise<FormatInfo[]>;
  }

  read(files: Record<string, Uint8Array>, options?: ReadOptions): Promise<ReadResult> {
    const call: Call = options ? { op: 'read', files, options } : { op: 'read', files };
    const result = this.call(call) as Promise<ReadResult>;
    // Remembered once it succeeds, so a read that crashed the worker is never
    // replayed into the next one.
    result.then(
      () => {
        this.lastRead = call;
      },
      () => {},
    );
    return result;
  }

  write(options: WriteOptions): Promise<WriteResult> {
    return this.call({ op: 'write', options }) as Promise<WriteResult>;
  }

  dispose(): void {
    this.crash('the converter was closed');
  }

  private ensure(next: Call): Worker {
    if (this.worker) return this.worker;
    const worker = this.createWorker();
    worker.onmessage = (e: MessageEvent<Reply>) => this.settle(e.data);
    worker.onerror = (e: ErrorEvent) => this.crash(e.message || 'the converter stopped');
    this.worker = worker;
    // A new worker holds no model: give it the last one before anything
    // else, unless the next call reads a new one anyway.
    if (this.lastRead && next.op !== 'read') this.post(this.lastRead, () => {}, () => {});
    return worker;
  }

  private call(call: Call): Promise<unknown> {
    return new Promise((resolve, reject) => {
      this.ensure(call);
      this.post(call, resolve, reject);
    });
  }

  private post(call: Call, resolve: Pending['resolve'], reject: Pending['reject']) {
    const id = this.next++;
    this.pending.set(id, { resolve, reject });
    // Input bytes go as copies (no transfer list): the page keeps showing them.
    this.worker!.postMessage({ ...call, id } as Request);
  }

  private settle(reply: Reply) {
    const waiting = this.pending.get(reply.id);
    if (!waiting) return;
    this.pending.delete(reply.id);
    if (reply.ok) {
      waiting.resolve(reply.value);
      return;
    }
    const { code, message, param, candidates } = reply.error;
    waiting.reject({ code: code as ClientError['code'], message, ...(param !== undefined ? { param } : {}), ...(candidates ? { candidates } : {}) });
    if (code === 'worker_crashed') this.crash(message);
  }

  private crash(message: string) {
    this.worker?.terminate();
    this.worker = null;
    const waiting = [...this.pending.values()];
    this.pending.clear();
    for (const w of waiting) w.reject({ code: 'worker_crashed', message });
  }
}
