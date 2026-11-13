import type { ReactNode } from 'react';
import { X, Plus, RefreshCw } from 'lucide-react';
import { layoutBox, wireValue } from './params';
import { useRecentGenerations } from './useRecentGenerations';

type AnySpec = { kind: string; [k: string]: unknown };
type Draft = Record<string, unknown>;

interface Props {
  /** The `data-testid` prefix the row's controls share. */
  tid: string;
  spec: AnySpec;
  /** The draft the control edits; `wireValue` turns it into what is sent. */
  value: unknown;
  onChange: (v: unknown) => void;
}

const asDraft = (v: unknown): Draft => (v != null && typeof v === 'object' && !Array.isArray(v) ? (v as Draft) : {});
const asDrafts = (v: unknown): Draft[] => (Array.isArray(v) ? v.filter(x => x != null && typeof x === 'object') as Draft[] : []);
const str = (v: unknown): string => (v == null ? '' : String(v));
const list = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []);
const maxOf = (spec: AnySpec, key = 'max_items'): number => (typeof spec[key] === 'number' ? (spec[key] as number) : Infinity);

/** Number fields side by side, each `${tid}-${key}`. */
function NumberFields({ tid, keys, draft, onChange, step = '1', min = 0, max, labels }: {
  tid: string;
  keys: string[];
  draft: Draft;
  onChange: (d: Draft) => void;
  step?: string;
  min?: number;
  max?: number;
  labels?: Record<string, string>;
}) {
  return (
    <div className="pg-structured-row">
      {keys.map(k => (
        <label key={k} className="pg-structured-num">
          <span>{labels?.[k] ?? k}</span>
          <input type="number" className="input" data-testid={`${tid}-${k}`} min={min} max={max} step={step}
            value={str(draft[k])} onChange={e => onChange({ ...draft, [k]: e.target.value })} />
        </label>
      ))}
    </div>
  );
}

/** One removable row of a list control. */
function Row({ tid, i, onRemove, children }: { tid: string; i: number; onRemove: () => void; children: ReactNode }) {
  return (
    <li className="pg-structured-item" data-testid={`${tid}-item-${i}`}>
      <div className="pg-structured-item-body">{children}</div>
      <button type="button" className="pg-ref-remove" data-testid={`${tid}-remove-${i}`} aria-label={`Remove ${i + 1}`}
        onClick={onRemove}>
        <X size={12} aria-hidden="true" />
      </button>
    </li>
  );
}

function AddButton({ tid, disabled, onAdd, label }: { tid: string; disabled: boolean; onAdd: () => void; label: string }) {
  return (
    <button type="button" className="btn btn-secondary pg-structured-add" data-testid={`${tid}-add`} disabled={disabled} onClick={onAdd}>
      <Plus size={12} aria-hidden="true" /> {label}
    </button>
  );
}

/** The list editors share this shape: rows, a cap, and add/remove. */
function useList(value: unknown, onChange: (v: unknown) => void, cap: number, blank: () => Draft) {
  const items = asDrafts(value);
  return {
    items,
    full: items.length >= cap,
    add: () => onChange([...items, blank()]),
    remove: (i: number) => onChange(items.filter((_, j) => j !== i)),
    set: (i: number, d: Draft) => onChange(items.map((x, j) => (j === i ? d : x))),
  };
}

/**
 * The control for a structured param kind — `region`, `expand`, `time_range`,
 * `palette`, `layout`, `animation_list`, `source_model`, `source_video`,
 * `segmentation_prompts` — each editing a draft that `wireValue` turns into
 * the wire form, so a half-filled one is simply not sent.
 */
export default function StructuredParamField({ tid, spec, value, onChange }: Props) {
  switch (spec.kind) {
    case 'region':
      return (
        <NumberFields tid={tid} keys={['x', 'y', 'w', 'h']} draft={asDraft(value)} onChange={onChange}
          labels={{ x: 'x (px)', y: 'y (px)', w: 'width', h: 'height' }} />
      );
    case 'expand':
      return (
        <NumberFields tid={tid} keys={['left', 'top', 'right', 'bottom']} draft={asDraft(value)} onChange={onChange}
          max={typeof spec.max_px === 'number' ? spec.max_px : undefined} />
      );
    case 'time_range':
      return (
        <NumberFields tid={tid} keys={['start_s', 'end_s']} draft={asDraft(value)} onChange={onChange} step="any"
          labels={{ start_s: 'from (s)', end_s: 'to (s)' }} />
      );
    case 'palette':
      return <PaletteControl tid={tid} spec={spec} value={value} onChange={onChange} />;
    case 'layout':
      return <LayoutControl tid={tid} spec={spec} value={value} onChange={onChange} />;
    case 'animation_list':
      return <AnimationsControl tid={tid} spec={spec} value={value} onChange={onChange} />;
    case 'source_model':
    case 'source_video':
      return <SourceControl tid={tid} spec={spec} value={value} onChange={onChange} />;
    case 'segmentation_prompts':
      return <SegmentationControl tid={tid} spec={spec} value={value} onChange={onChange} />;
    default:
      return null;
  }
}

function PaletteControl({ tid, spec, value, onChange }: Props) {
  const colours = list(value);
  const cap = maxOf(spec, 'max_colors');
  return (
    <div className="pg-structured">
      <ul className="pg-palette">
        {colours.map((c, i) => (
          <li key={i} className="pg-palette-item" data-testid={`${tid}-item-${i}`}>
            <input type="color" data-testid={`${tid}-color-${i}`} aria-label={`Colour ${i + 1}`}
              value={/^#[0-9a-f]{6}$/i.test(c) ? c : '#000000'}
              onChange={e => onChange(colours.map((x, j) => (j === i ? e.target.value : x)))} />
            <code>{c}</code>
            <button type="button" className="pg-ref-remove" data-testid={`${tid}-remove-${i}`} aria-label={`Remove colour ${i + 1}`}
              onClick={() => onChange(colours.filter((_, j) => j !== i))}>
              <X size={12} aria-hidden="true" />
            </button>
          </li>
        ))}
      </ul>
      <AddButton tid={tid} label="Colour" disabled={colours.length >= cap}
        onAdd={() => onChange([...colours, '#4a90d9'])} />
      {spec.forced === false && <span className="pg-param-description">A hint: the model prefers these colours.</span>}
    </div>
  );
}

function LayoutControl({ tid, spec, value, onChange }: Props) {
  const kinds = list(spec.kinds);
  const rows = useList(value, onChange, maxOf(spec), () => ({ kind: kinds[0] ?? 'object', x: '0.1', y: '0.1', w: '0.3', h: '0.3' }));
  return (
    <div className="pg-structured">
      <ul className="pg-structured-list">
        {rows.items.map((d, i) => (
          <Row key={i} tid={tid} i={i} onRemove={() => rows.remove(i)}>
            <div className="pg-structured-row">
              <select className="input" data-testid={`${tid}-kind-${i}`} aria-label={`Box ${i + 1} kind`} value={str(d.kind)}
                onChange={e => rows.set(i, { ...d, kind: e.target.value })}>
                {kinds.map(k => <option key={k} value={k}>{k}</option>)}
              </select>
              <input className="input" data-testid={`${tid}-text-${i}`}
                aria-label={d.kind === 'text' ? `Box ${i + 1} word` : `Box ${i + 1} prompt`}
                placeholder={d.kind === 'text' ? (spec.single_word_text ? 'one word' : 'text') : 'what goes here (optional)'}
                value={str(d.kind === 'text' ? d.text : d.prompt)}
                onChange={e => rows.set(i, { ...d, [d.kind === 'text' ? 'text' : 'prompt']: e.target.value })} />
            </div>
            <NumberFields tid={`${tid}-${i}`} keys={['x', 'y', 'w', 'h']} draft={d} step="0.05" max={1}
              onChange={next => rows.set(i, next)} />
            {!layoutBox(d) && (
              <span className="pg-structured-note" data-testid={`${tid}-unsent-${i}`}>
                Not sent: a box needs its x, y, w and h.
              </span>
            )}
          </Row>
        ))}
      </ul>
      <AddButton tid={tid} label="Box" disabled={rows.full} onAdd={rows.add} />
    </div>
  );
}

function AnimationsControl({ tid, spec, value, onChange }: Props) {
  const presets = asDrafts(spec.presets).map(p => str(p.id)).filter(Boolean);
  const textToMotion = spec.text_to_motion != null;
  const rows = useList(value, onChange, maxOf(spec), () => ({ preset: presets[0] ?? '', name: '' }));
  const sent = asDrafts(wireValue(spec, value));
  return (
    <div className="pg-structured">
      <ul className="pg-structured-list">
        {rows.items.map((d, i) => (
          <Row key={i} tid={tid} i={i} onRemove={() => rows.remove(i)}>
            <div className="pg-structured-row">
              {presets.length > 0 && (
                <select className="input" data-testid={`${tid}-preset-${i}`} aria-label={`Clip ${i + 1} preset`} value={str(d.preset)}
                  onChange={e => rows.set(i, { ...d, preset: e.target.value })}>
                  {textToMotion && <option value="">from a prompt</option>}
                  {presets.map(p => <option key={p} value={p}>{p}</option>)}
                </select>
              )}
              <input className="input" data-testid={`${tid}-name-${i}`} aria-label={`Clip ${i + 1} name`}
                placeholder="name (optional)" value={str(d.name)}
                onChange={e => rows.set(i, { ...d, name: e.target.value })} />
            </div>
            {textToMotion && !d.preset && (
              <div className="pg-structured-row">
                <input className="input" data-testid={`${tid}-prompt-${i}`} aria-label={`Clip ${i + 1} motion prompt`}
                  placeholder="describe the motion" value={str(d.prompt)}
                  onChange={e => rows.set(i, { ...d, prompt: e.target.value })} />
                <input type="number" className="input" data-testid={`${tid}-duration-${i}`} aria-label={`Clip ${i + 1} seconds`}
                  min={0} step="any" placeholder="seconds" value={str(d.duration_s)}
                  onChange={e => rows.set(i, { ...d, duration_s: e.target.value })} />
              </div>
            )}
            <div className="pg-structured-row">
              <label className="pg-param-set-item">
                <input type="checkbox" data-testid={`${tid}-loop-${i}`} checked={d.loop === true}
                  onChange={e => rows.set(i, { ...d, loop: e.target.checked })} /> loop
              </label>
              <label className="pg-param-set-item">
                <input type="checkbox" data-testid={`${tid}-in-place-${i}`} checked={d.in_place === true}
                  onChange={e => rows.set(i, { ...d, in_place: e.target.checked })} /> in place
              </label>
            </div>
          </Row>
        ))}
      </ul>
      <AddButton tid={tid} label="Clip" disabled={rows.full} onAdd={rows.add} />
      {rows.items.length > 0 && sent.length < rows.items.length && (
        <span className="pg-structured-note" data-testid={`${tid}-unsent`}>
          A clip needs a preset or a motion prompt; the others are not sent.
        </span>
      )}
    </div>
  );
}

function SourceControl({ tid, spec, value, onChange }: Props) {
  const isVideo = spec.kind === 'source_video';
  const draft = asDraft(value);
  const type = draft.type === 'url' ? 'url' : 'generation';
  const formats = list(spec.formats);
  const recent = useRecentGenerations(isVideo ? 'video' : 'model3d', type === 'generation');
  return (
    <div className="pg-structured">
      <div className="pg-structured-row">
        <select className="input" data-testid={`${tid}-type`} aria-label="Source kind" value={type}
          onChange={e => onChange({ type: e.target.value, value: '' })}>
          <option value="generation">a generation of mine</option>
          <option value="url">a URL</option>
        </select>
        {type === 'generation' && (
          <button type="button" className="btn btn-secondary" data-testid={`${tid}-refresh`} title="Reload my recent generations"
            aria-label="Reload my recent generations" onClick={recent.reload}>
            <RefreshCw size={12} aria-hidden="true" />
          </button>
        )}
      </div>
      {type === 'generation' ? (
        <>
          <select className="input" data-testid={`${tid}-pick`} aria-label={`Pick a completed ${isVideo ? 'video' : '3D'} generation`}
            value={recent.items.some(g => g.id === draft.value) ? str(draft.value) : ''}
            onChange={e => onChange({ type: 'generation', value: e.target.value })}>
            <option value="">{recent.loading ? 'loading…' : recent.items.length ? 'pick a completed generation' : 'no completed generations yet'}</option>
            {recent.items.map(g => (
              <option key={g.id} value={g.id}>{`${g.model} · ${new Date(g.created_at).toLocaleString()} · ${g.id.slice(0, 8)}`}</option>
            ))}
          </select>
          <input className="input" data-testid={`${tid}-value`} aria-label="Generation id" placeholder="or paste a generation id"
            value={str(draft.value)} onChange={e => onChange({ type: 'generation', value: e.target.value })} />
        </>
      ) : (
        <div className="pg-structured-row">
          <input type="url" className="input" data-testid={`${tid}-value`} aria-label="Source URL" placeholder="https://…"
            value={str(draft.value)} onChange={e => onChange({ ...draft, type: 'url', value: e.target.value })} />
          <select className="input" data-testid={`${tid}-format`} aria-label="Source format" value={str(draft.format) || formats[0] || ''}
            onChange={e => onChange({ ...draft, type: 'url', format: e.target.value })}>
            {formats.map(f => <option key={f} value={f}>{f}</option>)}
          </select>
        </div>
      )}
    </div>
  );
}

function SegmentationControl({ tid, spec, value, onChange }: Props) {
  const types = list(spec.types);
  const rows = useList(value, onChange, maxOf(spec), () => ({ type: types[0] ?? 'point' }));
  return (
    <div className="pg-structured">
      <ul className="pg-structured-list">
        {rows.items.map((d, i) => (
          <Row key={i} tid={tid} i={i} onRemove={() => rows.remove(i)}>
            <select className="input" data-testid={`${tid}-type-${i}`} aria-label={`Prompt ${i + 1} kind`} value={str(d.type)}
              onChange={e => rows.set(i, { type: e.target.value })}>
              {types.map(t => <option key={t} value={t}>{t}</option>)}
            </select>
            {d.type === 'text' ? (
              <input className="input" data-testid={`${tid}-text-${i}`} aria-label={`Prompt ${i + 1} text`} placeholder="what to select"
                value={str(d.text)} onChange={e => rows.set(i, { ...d, text: e.target.value })} />
            ) : d.type === 'box' ? (
              <NumberFields tid={`${tid}-${i}`} keys={['x', 'y', 'w', 'h']} draft={d} onChange={next => rows.set(i, next)} />
            ) : (
              <NumberFields tid={`${tid}-${i}`} keys={['x', 'y', 'label']} draft={d} onChange={next => rows.set(i, next)}
                labels={{ x: 'x (px)', y: 'y (px)', label: 'label 1 = keep, 0 = exclude' }} />
            )}
          </Row>
        ))}
      </ul>
      <AddButton tid={tid} label="Prompt" disabled={rows.full} onAdd={rows.add} />
    </div>
  );
}
