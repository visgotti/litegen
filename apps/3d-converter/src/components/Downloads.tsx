// The written files, as downloads: one file, the zip, or each file of a
// package (named as the model references them).
import { useEffect, useMemo } from 'react';
import { Archive, Download } from 'lucide-react';
import type { Conversion } from '@litegen/3d-converter';
import { formatBytes } from '@litegen/3d-viewer';

/** The download name: the input's own name for a lone model file, the
 *  package's names otherwise (the model's references expect them). */
export function downloadName(file: Conversion['files'][number], index: number, conversion: Conversion, stem: string): string {
  if (conversion.files.length > 1) return file.name;
  if (index === 0 && file.name === `model.${conversion.format}`) return `${stem}.${conversion.format}`;
  if (index === 0 && file.name === `model.${conversion.format}.zip`) return `${stem}.${conversion.format}.zip`;
  return file.name;
}

/** Whether a written file sits in a folder (`textures/…`). A browser saves a
 *  download flat, so such files lose the folder their model points at. */
export function foldersIn(conversion: Conversion): boolean {
  return conversion.files.length > 1 && conversion.files.some((f) => f.name.includes('/'));
}

export default function Downloads({ conversion, stem, onZip }: { conversion: Conversion; stem: string; onZip?: () => void }) {
  const urls = useMemo(() => conversion.files.map((f) => URL.createObjectURL(new Blob([f.data], { type: f.mediaType }))), [conversion]);
  useEffect(() => () => urls.forEach((u) => URL.revokeObjectURL(u)), [urls]);
  return (
    <div className="downloads">
      {conversion.files.map((f, i) => {
        const name = downloadName(f, i, conversion, stem);
        return (
          <a key={f.name} className="btn btn-primary download" href={urls[i]} download={name} data-testid={i === 0 ? 'download' : `download-${i}`}>
            <Download size={14} aria-hidden="true" /> {name} <span className="download-size">{formatBytes(f.data.length)}</span>
          </a>
        );
      })}
      {foldersIn(conversion) && onZip && (
        <p className="downloads-note" data-testid="downloads-folders">
          Saved one by one, files in folders lose their folder, and the model can't find them.{' '}
          <button type="button" className="btn btn-secondary" onClick={onZip} data-testid="download-zip">
            <Archive size={14} aria-hidden="true" /> Download as a zip
          </button>
        </p>
      )}
    </div>
  );
}
