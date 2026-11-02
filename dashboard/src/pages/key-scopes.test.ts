import { describe, expect, it } from 'vitest';
import { availablePresets, presetFor, scopeLabel } from './key-scopes';

describe('presetFor', () => {
  it('matches a preset regardless of order, spacing and duplicates', () => {
    expect(presetFor('read, generate ,admin')).toBe('ai_management');
    expect(presetFor('admin,read')).toBe('management');
    expect(presetFor('generate,generate')).toBe('ai');
    expect(presetFor(' read ')).toBe('read_only');
  });

  it('calls anything else custom', () => {
    // The API's own default for keys created without scopes is not a preset.
    expect(presetFor('generate,read')).toBe('custom');
    expect(presetFor('admin')).toBe('custom');
    expect(presetFor('')).toBe('custom');
  });
});

describe('scopeLabel', () => {
  it('names a preset and leaves other scope lists as written', () => {
    expect(scopeLabel('read,admin')).toBe('Management');
    expect(scopeLabel('generate')).toBe('AI');
    expect(scopeLabel('generate,read')).toBe('generate,read');
  });
});

describe('availablePresets', () => {
  it('offers the admin presets only to someone who can grant admin', () => {
    expect(availablePresets(false)).toEqual(['ai', 'read_only', 'custom']);
    expect(availablePresets(true)).toEqual(['ai', 'management', 'ai_management', 'read_only', 'custom']);
  });
});
