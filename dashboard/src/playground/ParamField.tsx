import type { ReactNode } from 'react';
import type { ParamSpec } from '@litegen/sdk';
import { paramDescription, paramLabel, sizeEnumOptions } from './params';

type AnySpec = { kind: string; [k: string]: unknown };

interface Props {
  name: string;
  spec: ParamSpec;
  models: string[];        // applicability
  totalSelected: number;   // to render "all" vs the model list
  value: unknown;
  onChange: (v: unknown) => void;
}

export default function ParamField({ name, spec, models, totalSelected, value, onChange }: Props) {
  const s = spec as unknown as AnySpec;
  const tid = `pg-param-${name}`;
  const applies = models.length === totalSelected
    ? 'all'
    : models.map(m => m.split('/').pop()).join(', ');

  let control: ReactNode;
  switch (s.kind) {
    case 'bool':
      control = (
        <input type="checkbox" data-testid={tid} checked={Boolean(value)}
          onChange={e => onChange(e.target.checked)} />
      );
      break;
    case 'int':
    case 'float':
      control = (
        <input type="number" className="input" data-testid={tid}
          min={s.min as number} max={s.max as number}
          step={s.kind === 'float' ? 'any' : 1}
          value={value as number | string} onChange={e => onChange(e.target.value)} />
      );
      break;
    case 'string': {
      const ev = (s.enum_values as string[]) ?? [];
      control = ev.length ? (
        <select className="input" data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {ev.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      ) : (
        <input type="text" className="input" data-testid={tid}
          maxLength={s.max_length as number}
          value={String(value ?? '')} onChange={e => onChange(e.target.value)} />
      );
      break;
    }
    case 'string_array': {
      // A set, not a choice — `output_formats` is the only one today, and every
      // member ticked here is a container the generation is required to deliver
      // or fail. Checkboxes rather than a multi-select: a native multi-select
      // hides the unselected options behind a scroll and needs ctrl-click to
      // deselect, which is a poor way to present a promise the caller pays for.
      const ev = (s.enum_values as string[]) ?? [];
      const max = s.max_items as number | undefined;
      const chosen = Array.isArray(value) ? (value as string[]) : [];
      // At the cap, the unticked boxes are disabled rather than silently
      // dropping a click — the backend would reject the request with
      // `param_too_many`, and finding that out after submitting is worse.
      const atCap = max != null && chosen.length >= max;
      control = (
        <div className="pg-param-set" data-testid={tid}>
          {ev.map(o => {
            const on = chosen.includes(o);
            return (
              <label key={o} className="pg-param-set-item">
                <input
                  type="checkbox"
                  data-testid={`${tid}-${o}`}
                  checked={on}
                  disabled={!on && atCap}
                  onChange={e => onChange(
                    // Filter from enum_values rather than pushing/splicing, so
                    // the submitted order always matches the declared order.
                    ev.filter(x => (x === o ? e.target.checked : chosen.includes(x))),
                  )}
                />
                {o}
              </label>
            );
          })}
          {max != null && max < ev.length && (
            <span className="pg-param-set-cap" data-testid={`${tid}-cap`}>
              up to {max}
            </span>
          )}
        </div>
      );
      break;
    }
    case 'aspect_ratio': {
      const allowed = (s.allowed as string[]) ?? [];
      control = (
        <select className="input" data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {allowed.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      );
      break;
    }
    case 'size': {
      const opts = sizeEnumOptions(spec);
      control = opts.length ? (
        <select className="input" data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {opts.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      ) : (
        <input type="text" className="input" data-testid={tid}
          placeholder="WxH" value={String(value ?? '')}
          onChange={e => onChange(e.target.value)} />
      );
      break;
    }
    case 'seed':
      control = (
        <input type="number" className="input" data-testid={tid}
          placeholder="Random" value={String(value ?? '')}
          onChange={e => onChange(e.target.value)} />
      );
      break;
    default:
      control = (
        <input type="text" className="input" data-testid={tid}
          value={String(value ?? '')} onChange={e => onChange(e.target.value)} />
      );
  }

  // The description is both the label's tooltip and a visible help line:
  // a tooltip alone never shows on touch devices.
  const description = paramDescription(spec);
  return (
    <div className="pg-param-row">
      <label className="pg-param-label" title={description ?? undefined}>
        {paramLabel(name, spec)}
        <span className="pg-param-applies" data-testid={`${tid}-applies`}>{applies}</span>
      </label>
      {control}
      {description && (
        <span className="pg-param-description" data-testid={`${tid}-description`}>{description}</span>
      )}
    </div>
  );
}
