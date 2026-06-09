import type { ReactNode } from 'react';
import type { ParamSpec } from '@litegen/sdk';
import { sizeEnumOptions } from './params';

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

  return (
    <div className="pg-param-row">
      <label className="pg-param-label">
        {name}
        <span className="pg-param-applies" data-testid={`${tid}-applies`}>{applies}</span>
      </label>
      {control}
    </div>
  );
}
