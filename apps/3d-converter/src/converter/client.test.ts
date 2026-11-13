import { describe, expect, it } from 'vitest';
import { ConverterClient, type ClientError } from './client';
import type { Request } from './protocol';

/** Stands in for the module worker: records what is posted (with its
 *  transfer list) and replies or crashes on command. */
class FakeWorker {
  static made: FakeWorker[] = [];
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: ((e: ErrorEvent) => void) | null = null;
  posted: Array<{ request: Request; transfer: Transferable[] }> = [];
  terminated = false;
  constructor() {
    FakeWorker.made.push(this);
  }
  postMessage(request: Request, transfer: Transferable[] = []) {
    this.posted.push({ request, transfer });
  }
  terminate() {
    this.terminated = true;
  }
  reply(id: number, value: unknown, transfer: ArrayBuffer[] = []) {
    void transfer;
    this.onmessage?.({ data: { id, ok: true, value } } as MessageEvent);
  }
  fail(id: number, error: { name: string; code: string; message: string; param?: string }) {
    this.onmessage?.({ data: { id, ok: false, error } } as MessageEvent);
  }
  crash(message: string) {
    this.onerror?.({ message } as ErrorEvent);
  }
}

const client = () => {
  FakeWorker.made = [];
  return new ConverterClient(() => new FakeWorker() as unknown as Worker);
};
const worker = (i = 0) => FakeWorker.made[i]!;

describe('ConverterClient', () => {
  it('matches replies to requests by id when they arrive out of order', async () => {
    const c = client();
    const first = c.formats();
    const second = c.read({ 'a.glb': new Uint8Array([1]) });
    const [r1, r2] = worker().posted.map((p) => p.request.id);
    worker().reply(r2!, { format: 'glb' });
    worker().reply(r1!, [{ format: 'glb' }]);
    await expect(first).resolves.toEqual([{ format: 'glb' }]);
    await expect(second).resolves.toEqual({ format: 'glb' });
  });

  it('rejects with the converter error code and param', async () => {
    const c = client();
    const p = c.write({ to: 'glb', units: 'mm' });
    worker().fail(worker().posted[0]!.request.id, { name: 'ConvertError', code: 'param_unsupported', message: '`units` applies …', param: 'units' });
    await expect(p).rejects.toEqual({ code: 'param_unsupported', message: '`units` applies …', param: 'units' } satisfies ClientError);
  });

  it('recovers after a worker crash, replaying the last read', async () => {
    const c = client();
    const files = { 'a.glb': new Uint8Array([1, 2]) };
    const read = c.read(files);
    worker().reply(worker().posted[0]!.request.id, { format: 'glb' });
    await read;
    const write = c.write({ to: 'obj' });
    worker().crash('RuntimeError: unreachable');
    await expect(write).rejects.toMatchObject({ code: 'worker_crashed' });
    expect(worker(0).terminated).toBe(true);
    const again = c.write({ to: 'obj' });
    expect(FakeWorker.made).toHaveLength(2);
    const fresh = worker(1).posted.map((p) => p.request);
    expect(fresh.map((r) => r.op)).toEqual(['read', 'write']);
    expect(fresh[0]).toMatchObject({ op: 'read', files });
    worker(1).reply(fresh[0]!.id, { format: 'glb' });
    worker(1).reply(fresh[1]!.id, { conversion: { format: 'obj' }, ms: 3 });
    await expect(again).resolves.toMatchObject({ conversion: { format: 'obj' } });
  });

  it('does not replay a read that crashed the worker', async () => {
    // Review Focus 5: a file that traps the reader must not take the next,
    // valid file down with it.
    const c = client();
    const bad = c.read({ 'bad.glb': new Uint8Array([9]) });
    worker().crash('RuntimeError: unreachable');
    await expect(bad).rejects.toMatchObject({ code: 'worker_crashed' });
    const formats = c.formats();
    const fresh = worker(1).posted.map((p) => p.request);
    expect(fresh.map((r) => r.op)).toEqual(['formats']);
    worker(1).reply(fresh[0]!.id, []);
    await expect(formats).resolves.toEqual([]);
  });

  it('gives a fresh worker a new read without replaying the old one first', async () => {
    const c = client();
    const first = c.read({ 'a.glb': new Uint8Array([1]) });
    worker().reply(worker().posted[0]!.request.id, { format: 'glb' });
    await first;
    const write = c.write({ to: 'obj' });
    worker().crash('RuntimeError: unreachable');
    await expect(write).rejects.toMatchObject({ code: 'worker_crashed' });
    const second = c.read({ 'b.glb': new Uint8Array([2]) });
    const fresh = worker(1).posted.map((p) => p.request);
    expect(fresh).toHaveLength(1);
    expect(fresh[0]).toMatchObject({ op: 'read', files: { 'b.glb': new Uint8Array([2]) } });
    worker(1).reply(fresh[0]!.id, { format: 'glb' });
    await expect(second).resolves.toMatchObject({ format: 'glb' });
  });

  it('treats a worker_crashed reply as a crash: the next call gets a new worker', async () => {
    const c = client();
    const p = c.formats();
    worker().fail(worker().posted[0]!.request.id, { name: 'RuntimeError', code: 'worker_crashed', message: 'unreachable' });
    await expect(p).rejects.toMatchObject({ code: 'worker_crashed' });
    void c.formats();
    expect(FakeWorker.made).toHaveLength(2);
  });

  it('sends input bytes as copies, so the page keeps showing them', () => {
    const c = client();
    void c.read({ 'a.glb': new Uint8Array([1]) });
    expect(worker().posted[0]!.transfer).toEqual([]);
  });
});
