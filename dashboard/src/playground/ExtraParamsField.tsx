import type { ReactNode } from 'react';
import type { MergedExtra } from './types';
import { extraValue } from './params';

interface Props {
  extras: MergedExtra[];
  /** Raw text per key, as typed. */
  values: Record<string, string>;
  totalSelected: number;
  onChange: (key: string, raw: string) => void;
  /** The vendor's API reference, when exactly one model is selected. */
  docsUrl?: string | null;
  disabled?: boolean;
}

type AnySpec = { kind: string; [k: string]: unknown };

/** "Provider default", with the default the catalog states, if any. */
function providerDefault(spec: AnySpec | undefined): string {
  const d = spec?.default;
  return d == null || (Array.isArray(d) && d.length === 0) ? 'Provider default' : `Provider default (${Array.isArray(d) ? d.join(', ') : String(d)})`;
}

/** What a value will go as, said plainly. */
function sendsAs(v: unknown): string {
  if (typeof v === 'string') return 'text';
  if (Array.isArray(v)) return 'a list';
  if (v === null) return 'null';
  return typeof v;
}

/**
 * Provider passthrough: the vendor fields a model allows in `extra` that no
 * canonical param maps to (Rodin's `quality` / `tier`, TRELLIS's `texture_size`,
 * …). The server forwards them verbatim, gated by each model's
 * `extra_allowlist`, so each model is sent only its own keys. A key the
 * catalog types (`extra_params`) gets a control of its type — a list of its
 * values, a number box, a yes/no — and goes as that type; an untyped one is
 * free text. Collapsed by default: these are expert knobs whose meaning lives
 * in the vendor's docs.
 */
export default function ExtraParamsField({ extras, values, totalSelected, onChange, docsUrl, disabled }: Props) {
  if (extras.length === 0) return null;
  const setCount = extras.filter(e => (values[e.key] ?? '').trim() !== '').length;
  return (
    <details className="pg-extras" data-testid="pg-extras" open={setCount > 0 || undefined}>
      <summary data-testid="pg-extras-toggle">
        Provider passthrough <code>extra</code>
        <span className="pg-param-applies">{setCount ? `${setCount} set` : `${extras.length} available`}</span>
      </summary>
      <p className="pg-param-description">
        Sent verbatim to the provider. A typed field goes as its type; for any other, a value that parses as JSON
        (<code>512</code>, <code>true</code>, <code>{'{"a":1}'}</code>) is sent as JSON, anything else as text.
        {docsUrl && (
          <>
            {' '}
            <a href={docsUrl} target="_blank" rel="noreferrer" data-testid="pg-extras-docs">Field reference ↗</a>
          </>
        )}
      </p>
      {extras.map(e => {
        const tid = `pg-extra-${e.key}`;
        const spec = e.spec as unknown as AnySpec | undefined;
        const raw = values[e.key] ?? '';
        const sent = extraValue(spec, raw);
        const applies = e.models.length === totalSelected ? 'all' : e.models.map(m => m.split('/').pop()).join(', ');
        const set = (v: string) => onChange(e.key, v);
        const listed = Array.isArray(spec?.enum_values) && spec.enum_values.length
          ? (spec.enum_values as unknown[]).map(String)
          : Array.isArray(spec?.values) && spec.values.length ? (spec.values as unknown[]).map(String) : [];
        let control: ReactNode;
        if (listed.length) {
          control = (
            <select className="input" id={`${tid}-input`} data-testid={tid} value={raw} disabled={disabled}
              onChange={ev => set(ev.target.value)}>
              <option value="">{providerDefault(spec)}</option>
              {listed.map(v => <option key={v} value={v}>{v}</option>)}
            </select>
          );
        } else if (spec?.kind === 'bool') {
          control = (
            <select className="input" id={`${tid}-input`} data-testid={tid} value={raw} disabled={disabled}
              onChange={ev => set(ev.target.value)}>
              <option value="">{providerDefault(spec)}</option>
              <option value="true">true</option>
              <option value="false">false</option>
            </select>
          );
        } else if (spec?.kind === 'int' || spec?.kind === 'float') {
          control = (
            <input type="number" className="input" id={`${tid}-input`} data-testid={tid}
              min={typeof spec.min === 'number' ? spec.min : undefined} max={typeof spec.max === 'number' ? spec.max : undefined}
              step={spec.kind === 'float' ? 'any' : 1} placeholder={providerDefault(spec)} value={raw} disabled={disabled}
              onChange={ev => set(ev.target.value)} />
          );
        } else {
          control = (
            <input type="text" className="input" id={`${tid}-input`} data-testid={tid}
              placeholder={spec?.kind === 'string_array' ? `${providerDefault(spec)}: a, b, …` : providerDefault(spec)}
              value={raw} disabled={disabled} onChange={ev => set(ev.target.value)} />
          );
        }
        return (
          <div key={e.key} className="pg-param-row">
            <div className="pg-param-head">
              <label className="pg-param-label" htmlFor={`${tid}-input`}><code>{e.key}</code></label>
              <span className="pg-param-applies" data-testid={`${tid}-applies`}>{applies}</span>
            </div>
            {control}
            {sent !== undefined && (
              <span className="pg-param-description" data-testid={`${tid}-parsed`}>sends {sendsAs(sent)}</span>
            )}
          </div>
        );
      })}
    </details>
  );
}
