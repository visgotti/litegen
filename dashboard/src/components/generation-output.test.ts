import { describe, expect, it } from 'vitest';
import type { RequestArtifact } from '@litegen/sdk';
import {
  MAX_NOT_FOUND_POLLS,
  MAX_TRANSIENT_ERROR_STREAK,
  httpStatusOf,
  isTerminalStatus,
  isTransientError,
  nextTransientStreak,
  resolvesViaGeneration,
  shouldKeepPolling,
  videoResultIsImage,
} from './generation-output';

describe('isTerminalStatus', () => {
  it.each(['completed', 'failed', 'cancelled'])('%s is terminal', s => {
    expect(isTerminalStatus(s)).toBe(true);
  });
  it.each(['pending', 'processing'])('%s is not terminal', s => {
    expect(isTerminalStatus(s)).toBe(false);
  });
});

describe('shouldKeepPolling', () => {
  it.each(['pending', 'processing'])('keeps polling a %s generation', status => {
    expect(shouldKeepPolling({ kind: 'generation', status })).toBe(true);
  });

  it.each(['completed', 'failed', 'cancelled'])('stops on a %s generation', status => {
    expect(shouldKeepPolling({ kind: 'generation', status })).toBe(false);
  });

  it('stops on a status it does not recognise rather than polling forever', () => {
    expect(shouldKeepPolling({ kind: 'generation', status: 'exploded' })).toBe(false);
  });

  it('keeps polling on a 404 within the retry budget — a safety margin for scheduling jitter', () => {
    expect(shouldKeepPolling({ kind: 'error', httpStatus: 404 }, 1)).toBe(true);
    expect(shouldKeepPolling({ kind: 'error', httpStatus: 404 }, MAX_NOT_FOUND_POLLS - 1)).toBe(true);
  });

  it('gives up once 404s persist past the not-found budget', () => {
    expect(shouldKeepPolling({ kind: 'error', httpStatus: 404 }, MAX_NOT_FOUND_POLLS)).toBe(false);
    expect(shouldKeepPolling({ kind: 'error', httpStatus: 404 }, MAX_NOT_FOUND_POLLS + 5)).toBe(false);
  });

  it.each([500, 502, 503, 408, 429, undefined])(
    'keeps polling a transient error (%s) while the streak is under the cap',
    httpStatus => {
      expect(shouldKeepPolling({ kind: 'error', httpStatus }, 0, 1)).toBe(true);
      expect(shouldKeepPolling({ kind: 'error', httpStatus }, 0, MAX_TRANSIENT_ERROR_STREAK - 1)).toBe(true);
    },
  );

  it('stops once the transient-error streak reaches the cap (3rd consecutive)', () => {
    expect(shouldKeepPolling({ kind: 'error', httpStatus: 500 }, 0, MAX_TRANSIENT_ERROR_STREAK)).toBe(false);
    expect(shouldKeepPolling({ kind: 'error' }, 0, MAX_TRANSIENT_ERROR_STREAK)).toBe(false);
  });

  it.each([400, 401, 403])('stops immediately on a non-transient 4xx (%s)', httpStatus => {
    expect(shouldKeepPolling({ kind: 'error', httpStatus }, 0, 1)).toBe(false);
  });
});

describe('isTransientError', () => {
  it.each([500, 502, 503, 408, 429, undefined])('treats %s as transient', httpStatus => {
    expect(isTransientError(httpStatus)).toBe(true);
  });

  it.each([400, 401, 403, 404])('does not treat %s as transient (404 has its own retry path)', httpStatus => {
    expect(isTransientError(httpStatus)).toBe(false);
  });
});

describe('nextTransientStreak', () => {
  it('increments on consecutive transient errors', () => {
    expect(nextTransientStreak({ kind: 'error', httpStatus: 500 }, 0)).toBe(1);
    expect(nextTransientStreak({ kind: 'error', httpStatus: 500 }, 1)).toBe(2);
    expect(nextTransientStreak({ kind: 'error' }, 2)).toBe(3);
  });

  it('a successful poll resets the streak to 0', () => {
    expect(nextTransientStreak({ kind: 'generation', status: 'processing' }, 2)).toBe(0);
  });

  it('a non-transient error resets the streak to 0', () => {
    expect(nextTransientStreak({ kind: 'error', httpStatus: 400 }, 2)).toBe(0);
  });

  it('a 404 resets the transient streak (it has its own separate streak)', () => {
    expect(nextTransientStreak({ kind: 'error', httpStatus: 404 }, 2)).toBe(0);
  });
});

describe('httpStatusOf', () => {
  it('reads the status of a LiteGenAPIError-shaped error', () => {
    expect(httpStatusOf(Object.assign(new Error('nf'), { status: 404 }))).toBe(404);
  });
  it.each([
    ['a plain Error', new Error('boom')],
    ['a non-numeric status', { status: '404' }],
    ['null', null],
    ['a string', 'nope'],
  ])('is undefined for %s', (_label, err) => {
    expect(httpStatusOf(err)).toBeUndefined();
  });
});

function artifact(over: Partial<RequestArtifact>): RequestArtifact {
  return {
    request_id: 'litegen-3d-1',
    media_type: 'model3d',
    prompt: 'a chair',
    negative_prompt: null,
    params_json: null,
    refs_meta_json: null,
    output_kind: 'url',
    output_value: null,
    output_mime: null,
    output_truncated: false,
    error_message: null,
    created_at: '2026-09-11T00:00:00Z',
    ...over,
  };
}

describe('resolvesViaGeneration', () => {
  it('resolves an async 3D artifact with no output URL', () => {
    expect(resolvesViaGeneration(artifact({ media_type: 'model3d' }))).toBe(true);
  });

  it('resolves an async video artifact with no output URL (null or empty)', () => {
    expect(resolvesViaGeneration(artifact({ media_type: 'video', output_value: null }))).toBe(true);
    expect(resolvesViaGeneration(artifact({ media_type: 'video', output_value: '' }))).toBe(true);
  });

  it('always resolves a 3D artifact — its asset list only lives on the generation row', () => {
    expect(resolvesViaGeneration(artifact({ media_type: 'model3d', output_value: 'https://x/mesh.glb' }))).toBe(true);
  });

  it('does not resolve a video artifact whose URL is already known', () => {
    expect(resolvesViaGeneration(artifact({ media_type: 'video', output_value: 'https://x/v.mp4' }))).toBe(false);
  });

  it('does not resolve images, errors, or base64 outputs', () => {
    expect(resolvesViaGeneration(artifact({ media_type: 'image' }))).toBe(false);
    expect(resolvesViaGeneration(artifact({ media_type: 'video', output_kind: 'error' }))).toBe(false);
    expect(resolvesViaGeneration(artifact({ media_type: 'model3d', output_kind: 'b64' }))).toBe(false);
  });
});

describe('videoResultIsImage', () => {
  it('trusts an image/* mime (the VisualTab GIF case)', () => {
    expect(videoResultIsImage('/mock/video/abc', 'image/gif')).toBe(true);
  });
  it('does not treat a video mime as an image', () => {
    expect(videoResultIsImage('https://x/a.gif', 'video/mp4')).toBe(false);
  });
  it.each([
    'https://cdn.example/out/clip.gif',
    'https://cdn.example/out/CLIP.GIF?sig=abc',
    'https://cdn.example/out/clip.webp#t=1',
    'data:image/gif;base64,R0lGOD',
  ])('recognises an image result by URL: %s', url => {
    expect(videoResultIsImage(url)).toBe(true);
  });
  it.each([
    'https://cdn.example/out/clip.mp4',
    'https://cdn.example/out/clip',
    '/mock/video/abc',
    'data:video/mp4;base64,AAAA',
  ])('treats %s as video', url => {
    expect(videoResultIsImage(url)).toBe(false);
  });
  it('is false with no URL', () => {
    expect(videoResultIsImage(null)).toBe(false);
    expect(videoResultIsImage(undefined)).toBe(false);
  });
});
