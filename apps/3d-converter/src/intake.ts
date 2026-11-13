// What the user dropped or picked, as the converter package takes it: file
// bytes by relative path. A dropped folder keeps its paths, so a model's
// references (an OBJ's `textures/…`) still resolve; the package resolves
// the rest by file name.
export interface FileLike {
  name: string;
  webkitRelativePath?: string;
  arrayBuffer(): Promise<ArrayBuffer>;
}

/** The File and Directory Entries API's entries (webkitGetAsEntry), as far
 *  as a drop needs them. */
export interface EntryLike {
  isFile: boolean;
  isDirectory: boolean;
  name: string;
  fullPath: string;
  file?(ok: (f: FileLike) => void, err?: (e: unknown) => void): void;
  createReader?(): { readEntries(ok: (entries: EntryLike[]) => void, err?: (e: unknown) => void): void };
}

export async function filesFromList(list: ArrayLike<FileLike>): Promise<Record<string, Uint8Array>> {
  const out: Record<string, Uint8Array> = {};
  for (const f of Array.from(list)) out[f.webkitRelativePath || f.name] = new Uint8Array(await f.arrayBuffer());
  return out;
}

function readAll(dir: EntryLike): Promise<EntryLike[]> {
  // readEntries returns a batch at a time, then an empty one.
  return new Promise((resolve, reject) => {
    const reader = dir.createReader!();
    const all: EntryLike[] = [];
    const next = () =>
      reader.readEntries((batch) => {
        if (batch.length === 0) return resolve(all);
        all.push(...batch);
        next();
      }, reject);
    next();
  });
}

export async function filesFromEntries(entries: EntryLike[]): Promise<Record<string, Uint8Array>> {
  const out: Record<string, Uint8Array> = {};
  const walk = async (entry: EntryLike): Promise<void> => {
    if (entry.isDirectory) {
      for (const child of await readAll(entry)) await walk(child);
    } else if (entry.isFile && entry.file) {
      const file = await new Promise<FileLike>((resolve, reject) => entry.file!(resolve, reject));
      out[entry.fullPath.replace(/^\/+/, '')] = new Uint8Array(await file.arrayBuffer());
    }
  };
  for (const e of entries) await walk(e);
  return out;
}

/** A drop's files: folders walked when the browser exposes entries. */
export async function filesFromDataTransfer(dt: DataTransfer): Promise<Record<string, Uint8Array>> {
  const entries = Array.from(dt.items)
    .map((item) => (item.kind === 'file' ? (item.webkitGetAsEntry?.() as unknown as EntryLike | null) : null))
    .filter((e): e is EntryLike => e != null);
  if (entries.length > 0) return filesFromEntries(entries);
  return filesFromList(dt.files);
}
