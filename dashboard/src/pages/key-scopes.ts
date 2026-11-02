// API-key scope presets, the way LiteLLM offers key types (llm_api,
// management, read_only, default). A preset is only a name for a scope list:
// the server still decides what each scope opens and who may grant it.

export type ScopePreset = 'ai' | 'management' | 'ai_management' | 'read_only' | 'custom';

type NamedPreset = Exclude<ScopePreset, 'custom'>;

export const PRESETS: Record<NamedPreset, { label: string; scopes: string; description: string; needsAdmin: boolean }> = {
  ai: {
    label: 'AI',
    scopes: 'generate',
    description: 'Generate images, video and 3D, and read models and its own generations. No management access.',
    needsAdmin: false,
  },
  management: {
    label: 'Management',
    scopes: 'read,admin',
    description: 'Manage keys, budgets, end users and settings, and read spend and logs. Cannot generate.',
    needsAdmin: true,
  },
  ai_management: {
    label: 'AI + management',
    scopes: 'generate,read,admin',
    description: "Everything this key's org allows.",
    needsAdmin: true,
  },
  read_only: {
    label: 'Read-only',
    scopes: 'read',
    description: 'Read models, generations, logs, spend and limits. Changes nothing.',
    needsAdmin: false,
  },
};

const ORDER: NamedPreset[] = ['ai', 'management', 'ai_management', 'read_only'];

/** The scope list as a sorted set, so order, spacing and repeats don't matter. */
function normalize(scopes: string): string {
  return [...new Set(scopes.split(',').map(s => s.trim()).filter(Boolean))].sort().join(',');
}

export function presetFor(scopes: string): ScopePreset {
  const wanted = normalize(scopes);
  return ORDER.find(p => normalize(PRESETS[p].scopes) === wanted) ?? 'custom';
}

export function scopeLabel(scopes: string): string {
  const preset = presetFor(scopes);
  return preset === 'custom' ? scopes : PRESETS[preset].label;
}

export function availablePresets(canGrantAdmin: boolean): ScopePreset[] {
  return [...ORDER.filter(p => canGrantAdmin || !PRESETS[p].needsAdmin), 'custom'];
}
