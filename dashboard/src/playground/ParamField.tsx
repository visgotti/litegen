import type { ReactNode } from 'react';
import type { ParamSpec } from '@litegen/sdk';
import { allShareOneJob, canAddToSet, derivableValues, paramDescription, paramLabel, sizeEnumOptions } from './params';

type AnySpec = { kind: string; [k: string]: unknown };

interface Props {
  name: string;
  spec: ParamSpec;
  models: string[];        // applicability
  totalSelected: number;   // to render "all" vs the model list
  value: unknown;
  onChange: (v: unknown) => void;
  /** Present when the user has set this param: clears it back to the model default. */
  onReset?: () => void;
}

/** Selects whose spec declares no default lead with this option: picking it
 *  sends nothing, so each model applies its own default. */
const MODEL_DEFAULT = 'Model default';

export default function ParamField({ name, spec, models, totalSelected, value, onChange, onReset }: Props) {
  const s = spec as unknown as AnySpec;
  const tid = `pg-param-${name}`;
  const applies = models.length === totalSelected
    ? 'all'
    : models.map(m => m.split('/').pop()).join(', ');
  const hasDefault = s.default != null;

  let control: ReactNode;
  switch (s.kind) {
    case 'bool':
      control = (
        <input type="checkbox" id={`${tid}-input`} data-testid={tid} checked={Boolean(value)}
          onChange={e => onChange(e.target.checked)} />
      );
      break;
    case 'int':
    case 'float': {
      const range = s.min != null || s.max != null ? `${s.min ?? ''}–${s.max ?? ''}` : '';
      control = (
        <input type="number" className="input" id={`${tid}-input`} data-testid={tid}
          min={s.min as number} max={s.max as number}
          step={s.kind === 'float' ? 'any' : 1}
          placeholder={range ? `${MODEL_DEFAULT} (${range})` : MODEL_DEFAULT}
          value={value as number | string} onChange={e => onChange(e.target.value)} />
      );
      break;
    }
    case 'string': {
      const ev = (s.enum_values as string[]) ?? [];
      control = ev.length ? (
        <select className="input" id={`${tid}-input`} data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {!hasDefault && <option value="">{MODEL_DEFAULT}</option>}
          {ev.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      ) : (
        <input type="text" className="input" id={`${tid}-input`} data-testid={tid}
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
      const shared = derivableValues(s);
      const chosen = Array.isArray(value) ? (value as string[]) : [];
      control = (
        <div className="pg-param-set" data-testid={tid}>
          {ev.map(o => {
            const on = chosen.includes(o);
            // An unticked box is disabled when ticking it would exceed the
            // provider-job budget, rather than silently dropping the click —
            // the backend would reject the request with `param_too_many`, and
            // finding that out after submitting is worse.
            const blocked = !on && !canAddToSet(s, chosen, o);
            return (
              <label key={o} className="pg-param-set-item" title={blocked ? 'Needs another provider job' : undefined}>
                <input
                  type="checkbox"
                  data-testid={`${tid}-${o}`}
                  checked={on}
                  disabled={blocked}
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
          {max != null && (
            <span className="pg-param-set-cap" data-testid={`${tid}-cap`}>
              {allShareOneJob(s)
                ? 'any combination is one provider job'
                : shared.length > 1
                  ? `${max} provider job${max === 1 ? '' : 's'} · ${shared.join('/')} share one`
                  : max < ev.length ? `up to ${max}` : null}
            </span>
          )}
        </div>
      );
      break;
    }
    case 'aspect_ratio': {
      const allowed = (s.allowed as string[]) ?? [];
      control = (
        <select className="input" id={`${tid}-input`} data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {!hasDefault && <option value="">{MODEL_DEFAULT}</option>}
          {allowed.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      );
      break;
    }
    case 'size': {
      const opts = sizeEnumOptions(spec);
      control = opts.length ? (
        <select className="input" id={`${tid}-input`} data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          <option value="">{MODEL_DEFAULT}</option>
          {opts.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      ) : (
        <input type="text" className="input" id={`${tid}-input`} data-testid={tid}
          placeholder={`WxH (${MODEL_DEFAULT.toLowerCase()} if blank)`} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)} />
      );
      break;
    }
    case 'seed':
      control = (
        <input type="number" className="input" id={`${tid}-input`} data-testid={tid}
          placeholder="Random" value={String(value ?? '')}
          onChange={e => onChange(e.target.value)} />
      );
      break;
    default:
      control = (
        <input type="text" className="input" id={`${tid}-input`} data-testid={tid}
          value={String(value ?? '')} onChange={e => onChange(e.target.value)} />
      );
  }

  // The description is both the label's tooltip and a visible help line:
  // a tooltip alone never shows on touch devices. The reset button and the
  // applicability note sit OUTSIDE the <label>: a button inside a label with no
  // `for` becomes the label's control, so clicking the name would reset it.
  const description = paramDescription(spec);
  const isSet = s.kind === 'string_array';
  return (
    <div className="pg-param-row" role={isSet ? 'group' : undefined} aria-labelledby={isSet ? `${tid}-label` : undefined}>
      <div className="pg-param-head">
        <label className="pg-param-label" id={`${tid}-label`} htmlFor={isSet ? undefined : `${tid}-input`}
          title={description ?? undefined}>
          {paramLabel(name, spec)}
        </label>
        <span className="pg-param-label-end">
          {onReset && (
            <button type="button" className="pg-param-reset" data-testid={`${tid}-reset`}
              title="Use the model's default" onClick={onReset}>reset</button>
          )}
          <span className="pg-param-applies" data-testid={`${tid}-applies`}>{applies}</span>
        </span>
      </div>
      {control}
      {description && (
        <span className="pg-param-description" data-testid={`${tid}-description`}>{description}</span>
      )}
    </div>
  );
}
