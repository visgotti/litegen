import { describe, expect, expectTypeOf, it } from 'vitest';
import { formats, type FileRole, type Format, type StoredFrame, type UpAxis } from '@litegen/3d-converter';
import {
  VIEWER_FORMATS,
  type StoredFrame as ViewerStoredFrame,
  type UpAxis as ViewerUpAxis,
  type ViewerFileRole,
  type ViewerFormat,
} from '@litegen/3d-viewer';

// The shared viewer names formats, file roles and frames itself, so the
// dashboard, which doesn't carry the converter, can use it. These pin its
// words to the converter's, so a format or role the converter gains fails
// here, not in a browser. The type assertions are checked by `tsc -b`.
describe("the viewer package's vocabulary is the converter's", () => {
  it('draws exactly the formats the converter writes', () => {
    expect([...VIEWER_FORMATS].sort()).toEqual(formats().map((f) => f.format).sort());
    expectTypeOf<ViewerFormat>().toEqualTypeOf<Format>();
  });

  it('names the same file roles, stored frames and up axes', () => {
    expectTypeOf<ViewerFileRole>().toEqualTypeOf<FileRole>();
    expectTypeOf<ViewerStoredFrame>().toEqualTypeOf<StoredFrame>();
    expectTypeOf<ViewerUpAxis>().toEqualTypeOf<UpAxis>();
  });
});
