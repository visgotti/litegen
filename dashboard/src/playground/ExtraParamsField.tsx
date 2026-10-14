import type { MergedExtra } from './types';
import { parseExtraValue } from './params';

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

/**
 * Provider passthrough: the vendor fields a model allows in `extra` that no
 * canonical param maps to (Rodin's `quality` / `tier`, TRELLIS's `texture_size`,
 * …). The server forwards them verbatim, gated by each model's
 * `extra_allowlist`, so each model is sent only its own keys. Collapsed by
 * default: these are expert knobs whose meaning lives in the vendor's docs.
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
        Sent verbatim to the provider. A value that parses as JSON (<code>512</code>, <code>true</code>,{' '}
        <code>{'{"a":1}'}</code>) is sent as JSON; anything else as text.
        {docsUrl && (
          <>
            {' '}
            <a href={docsUrl} target="_blank" rel="noreferrer" data-testid="pg-extras-docs">Field reference ↗</a>
          </>
        )}
      </p>
      {extras.map(e => {
        const tid = `pg-extra-${e.key}`;
        const raw = values[e.key] ?? '';
        const parsed = parseExtraValue(raw);
        const applies = e.models.length === totalSelected ? 'all' : e.models.map(m => m.split('/').pop()).join(', ');
        return (
          <div key={e.key} className="pg-param-row">
            <div className="pg-param-head">
              <label className="pg-param-label" htmlFor={`${tid}-input`}><code>{e.key}</code></label>
              <span className="pg-param-applies" data-testid={`${tid}-applies`}>{applies}</span>
            </div>
            <input type="text" className="input" id={`${tid}-input`} data-testid={tid}
              placeholder="Provider default" value={raw} disabled={disabled}
              onChange={ev => onChange(e.key, ev.target.value)} />
            {parsed !== undefined && (
              <span className="pg-param-description" data-testid={`${tid}-parsed`}>
                sends {typeof parsed === 'string' ? 'text' : Array.isArray(parsed) ? 'an array' : parsed === null ? 'null' : typeof parsed}
              </span>
            )}
          </div>
        );
      })}
    </details>
  );
}
