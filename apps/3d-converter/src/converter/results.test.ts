import { describe, expect, it } from 'vitest';
import type { ModelInfo } from '@litegen/3d-converter';
import { measured, modelCandidates, readResult } from './results';

const info = { triangles: 12 } as ModelInfo;

describe('readResult', () => {
  it('passes a measured model through', () => {
    const model = { format: 'glb' as const, info, warnings: ['w'], files: [] };
    expect(readResult(model)).toEqual({ format: 'glb', info, warnings: ['w'], files: [] });
  });

  it('keeps a model it cannot measure, with the reason', () => {
    // A model past the converter's merged-mesh cap still converts; only
    // measuring it fails.
    const model = {
      format: 'glb' as const,
      warnings: [],
      files: [],
      get info(): ModelInfo {
        throw new Error('the merged mesh passes 4000000 vertices');
      },
    };
    expect(readResult(model)).toEqual({ format: 'glb', infoError: 'the merged mesh passes 4000000 vertices', warnings: [], files: [] });
  });
});

describe('measured', () => {
  it('measures, or says why not', () => {
    expect(measured({ info })).toEqual({ info });
    const unmeasurable = {
      get info(): ModelInfo {
        throw new Error('too big');
      },
    };
    expect(measured(unmeasurable)).toEqual({ infoError: 'too big' });
  });
});

describe('modelCandidates', () => {
  it('lists the files the package recognises as models, by name', () => {
    const files = { 'b.fbx': new Uint8Array([2]), 'tex.png': new Uint8Array([3]), 'a.obj': new Uint8Array([1]) };
    const detect = (_: Uint8Array, name?: string) => (name?.endsWith('.png') ? undefined : 'obj');
    expect(modelCandidates(files, detect)).toEqual(['a.obj', 'b.fbx']);
  });
});
