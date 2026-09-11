import { describe, expect, it } from 'vitest';
import type { ParamSpec } from '@litegen/sdk';
import { paramDescription, paramLabel } from './params';

const spec = (extra: Record<string, unknown>): ParamSpec =>
  ({ kind: 'int', min: 100, max: 300000, ...extra }) as unknown as ParamSpec;

describe('paramLabel', () => {
  it('uses the declared label when present', () => {
    expect(paramLabel('target_polycount', spec({ label: 'Triangle budget' }))).toBe('Triangle budget');
  });

  it('humanises the key when the label is absent', () => {
    expect(paramLabel('target_polycount', spec({}))).toBe('Target polycount');
  });

  it('humanises the key when the label is null', () => {
    expect(paramLabel('target_polycount', spec({ label: null }))).toBe('Target polycount');
  });

  it('treats an empty or whitespace-only label as absent', () => {
    expect(paramLabel('guidance_scale', spec({ label: '' }))).toBe('Guidance scale');
    expect(paramLabel('guidance_scale', spec({ label: '   ' }))).toBe('Guidance scale');
  });

  it('trims surrounding whitespace from a declared label', () => {
    expect(paramLabel('steps', spec({ label: '  Steps  ' }))).toBe('Steps');
  });

  it('humanises a multi-part snake_case key, lowercasing all but the first word', () => {
    expect(paramLabel('negative_prompt_strength', spec({}))).toBe('Negative prompt strength');
  });

  it('capitalises a single-word key', () => {
    expect(paramLabel('topology', spec({}))).toBe('Topology');
  });

  it('ignores a non-string label from an unvalidated payload', () => {
    expect(paramLabel('symmetry', spec({ label: 42 }))).toBe('Symmetry');
  });
});

describe('paramDescription', () => {
  it('returns the trimmed description when present', () => {
    expect(paramDescription(spec({ description: ' Approximate triangle budget. ' })))
      .toBe('Approximate triangle budget.');
  });

  it('returns null when the description is absent, null, empty or not a string', () => {
    expect(paramDescription(spec({}))).toBeNull();
    expect(paramDescription(spec({ description: null }))).toBeNull();
    expect(paramDescription(spec({ description: '  ' }))).toBeNull();
    expect(paramDescription(spec({ description: 7 }))).toBeNull();
  });
});
