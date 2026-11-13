// The pieces both apps put below the inspector's stats: facts, clips, file
// and asset tables, notes, thumbnails, the error panel and the copy button.
// Each app maps its own data to them: the dashboard the API's assets and
// scene manifest, the converter tool the converter package's ModelInfo.
import { useEffect, useRef, useState, type ComponentType, type ReactNode } from 'react';
import { Check, Copy, TriangleAlert } from 'lucide-react';
import { copyToClipboard } from '../format';
// The parts are styled by the inspector's sheet, wherever they render.
import './inspector.css';

/** A coloured label: an asset's kind, a file's role, a loss's kind. */
export function Chip({ kind, children }: { kind: string; children: ReactNode }) {
  return <span className={`mp-kind mp-kind--${kind}`}>{children}</span>;
}

/** What litegen measured in a model, as label and value rows. */
export function FactList({ rows, testId, title = 'Model' }: { rows: Array<[string, string]>; testId: string; title?: string }) {
  if (rows.length === 0) return null;
  return (
    <>
      <h4 className="mp-section-title">{title}</h4>
      <dl className="mp-stats mp-scene" data-testid={`${testId}-facts`}>
        {rows.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
    </>
  );
}

export interface TableColumn {
  label: string;
  /** Figures: tabular digits. */
  numeric?: boolean;
  /** Read out but not shown (a links column). */
  hidden?: boolean;
}

export interface TableRow {
  key: string;
  cells: ReactNode[];
  testId?: string;
}

/** A titled table in the inspector's style, its row count in the title and
 *  in `data-count`, or `empty` in its place when it has no rows. */
export function DataTable({ title, testId, columns, rows, empty }: {
  title: string;
  testId: string;
  columns: TableColumn[];
  rows: TableRow[];
  empty?: ReactNode;
}) {
  const titleId = `${testId}-title`;
  return (
    <>
      <h4 className="mp-section-title" id={titleId}>
        {title} ({rows.length})
      </h4>
      {rows.length === 0 && empty ? (
        <p className="mp-empty" data-testid={testId} data-count={0}>
          {empty}
        </p>
      ) : (
        <div className="mp-assets-wrap">
          <table className="mp-assets" data-testid={testId} data-count={rows.length} aria-labelledby={titleId}>
            <thead>
              <tr>
                {columns.map((c) => (
                  <th key={c.label} scope="col">
                    {c.hidden ? <span className="mp-sr">{c.label}</span> : c.label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.key} data-testid={r.testId}>
                  {r.cells.map((cell, i) => (
                    <td key={i} className={columns[i]?.numeric ? 'mp-num' : columns[i]?.hidden ? 'mp-links' : undefined}>
                      {cell}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

export interface ClipRow {
  name: string;
  duration: string;
  /** Whether the clip was asked to loop; the column shows when any row has it. */
  loop?: string;
  inPlace: string;
  travel: string;
}

/** A model's animation clips, one row each. */
export function ClipTable({ rows, testId }: { rows: ClipRow[]; testId: string }) {
  if (rows.length === 0) return null;
  const loop = rows.some((r) => r.loop !== undefined);
  const columns: TableColumn[] = [
    { label: 'Clip' },
    { label: 'Length', numeric: true },
    ...(loop ? [{ label: 'Loop' }] : []),
    { label: 'In place' },
    { label: 'Root travel', numeric: true },
  ];
  return (
    <DataTable
      title="Clips"
      testId={`${testId}-clips`}
      columns={columns}
      rows={rows.map((c, i) => ({
        key: `${i}:${c.name}`,
        testId: `${testId}-clip-${i}`,
        cells: [c.name, c.duration, ...(loop ? [c.loop ?? '—'] : []), c.inPlace, c.travel],
      }))}
    />
  );
}

/** Lines of text a reader or writer left about the model: what it could not
 *  keep, what it skipped. */
export function NoteList({ title, notes, testId }: { title: string; notes: string[]; testId: string }) {
  if (notes.length === 0) return null;
  return (
    <>
      <h4 className="mp-section-title">
        {title} ({notes.length})
      </h4>
      <ul className="mp-warnings" data-testid={testId}>
        {notes.map((n, i) => (
          <li key={i}>{n}</li>
        ))}
      </ul>
    </>
  );
}

export interface Thumbnail {
  url: string;
  /** Shown under the image: the texture's or preview's kind or name. */
  label: string;
  alt: string;
}

const PlainImage = ({ src, alt }: { src: string; alt: string }) => <img src={src} alt={alt} loading="lazy" />;

/** The model's textures and 2D preview as small images, each a link to the
 *  full file. `image` draws one, for a caller whose files need fetching
 *  first (the dashboard's private file route). */
export function Thumbnails({ items, testId, image: Image = PlainImage }: {
  items: Thumbnail[];
  testId: string;
  image?: ComponentType<{ src: string; alt: string }>;
}) {
  if (items.length === 0) return null;
  return (
    <ul className="mp-thumbs" data-testid={`${testId}-thumbs`} aria-label="Preview and textures">
      {items.map((t, i) => (
        <li key={`${i}:${t.url}`}>
          <a href={t.url} target="_blank" rel="noreferrer" data-testid={`${testId}-thumb-${i}`}>
            <Image src={t.url} alt={t.alt} />
            <span aria-hidden="true">{t.label}</span>
          </a>
        </li>
      ))}
    </ul>
  );
}

/** In place of the viewer, when there is no model to show at all. */
export function InspectorError({ title, message, testId }: { title: string; message: string; testId: string }) {
  return (
    <div className="mp-error" role="alert" data-testid={`${testId}-error`}>
      <TriangleAlert size={16} aria-hidden="true" />
      <div>
        <strong>{title}</strong>
        <p>{message}</p>
      </div>
    </div>
  );
}

type CopyState = 'idle' | 'copied' | 'failed';

/** Copies a URL, saying for two seconds whether it worked. */
export function CopyUrlButton({ url, testId, label = 'Copy URL' }: { url: string; testId: string; label?: string }) {
  const [state, setState] = useState<CopyState>('idle');
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = async () => {
    const ok = await copyToClipboard(url, typeof navigator === 'undefined' ? undefined : navigator.clipboard);
    setState(ok ? 'copied' : 'failed');
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState('idle'), 2000);
  };
  // The visible text is the accessible name: no aria-label to drift from it.
  const text = state === 'copied' ? 'Copied' : state === 'failed' ? 'Copy failed' : label;
  return (
    <>
      <button type="button" className="btn btn-secondary mp-btn" title={url} data-testid={testId} data-copy-state={state} onClick={copy}>
        {state === 'copied' ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
        {text}
      </button>
      <span className="mp-sr" role="status" aria-live="polite">
        {state === 'copied' ? 'URL copied' : state === 'failed' ? 'Could not copy the URL' : ''}
      </span>
    </>
  );
}
