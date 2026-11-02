import { availablePresets, PRESETS, type ScopePreset } from '../pages/key-scopes';

interface Props {
  /** The selected preset; `custom` when the scopes were typed. */
  preset: ScopePreset;
  canGrantAdmin: boolean;
  /** Called with the chosen preset and, for a named preset, its scopes. */
  onPick: (preset: ScopePreset, scopes: string | null) => void;
  testId: string;
}

/** LiteLLM-style key types as radio buttons above a key's scopes field. */
export default function ScopePresetPicker({ preset, canGrantAdmin, onPick, testId }: Props) {
  const options = availablePresets(canGrantAdmin);
  const hint = preset === 'custom' ? 'Type the scopes: generate, read and/or admin.' : PRESETS[preset].description;
  return (
    <div>
      <div role="radiogroup" aria-label="Key access" data-testid={testId} style={{ display: 'flex', flexWrap: 'wrap', gap: 12 }}>
        {options.map(p => (
          <label key={p} style={{ display: 'flex', alignItems: 'center', gap: 4, fontSize: 13, cursor: 'pointer' }}>
            <input
              type="radio"
              name={testId}
              value={p}
              data-testid={`${testId}-${p}`}
              checked={preset === p}
              onChange={() => onPick(p, p === 'custom' ? null : PRESETS[p].scopes)}
            />
            {p === 'custom' ? 'Custom' : PRESETS[p].label}
          </label>
        ))}
      </div>
      <div style={{ color: '#8b949e', fontSize: 12, marginTop: 4 }}>{hint}</div>
    </div>
  );
}
