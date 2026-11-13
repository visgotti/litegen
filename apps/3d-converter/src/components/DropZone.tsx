// Where a model comes in: drop files or a folder, or pick them. Several
// files are one model and the files it references; a zip is unpacked by
// the converter.
import { useRef, useState, type DragEvent } from 'react';
import { FolderOpen, Upload } from 'lucide-react';
import { filesFromDataTransfer, filesFromList } from '../intake';

export default function DropZone({ onFiles, busy }: { onFiles: (files: Record<string, Uint8Array>) => void; busy: boolean }) {
  const [over, setOver] = useState(false);
  const pick = useRef<HTMLInputElement | null>(null);
  const folder = useRef<HTMLInputElement | null>(null);
  const onDrop = async (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    setOver(false);
    const files = await filesFromDataTransfer(e.dataTransfer);
    if (Object.keys(files).length > 0) onFiles(files);
  };
  const onPicked = async (list: FileList | null) => {
    if (!list || list.length === 0) return;
    onFiles(await filesFromList(list));
  };
  return (
    <div
      className={`drop${over ? ' drop--over' : ''}`}
      data-testid="drop-zone"
      onDragOver={(e) => {
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={onDrop}
    >
      <Upload size={22} aria-hidden="true" />
      <p className="drop-title">Drop a 3D model here</p>
      <p className="drop-hint">glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz, usda or usdc — with its .bin, .mtl and textures, a folder, or a zip.</p>
      <div className="drop-actions">
        <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => pick.current?.click()}>
          Choose files
        </button>
        <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => folder.current?.click()}>
          <FolderOpen size={14} aria-hidden="true" /> Choose a folder
        </button>
      </div>
      <input ref={pick} data-testid="file-input" type="file" multiple hidden onChange={(e) => void onPicked(e.target.files)} />
      <input ref={folder} type="file" hidden {...{ webkitdirectory: '' }} onChange={(e) => void onPicked(e.target.files)} />
    </div>
  );
}
