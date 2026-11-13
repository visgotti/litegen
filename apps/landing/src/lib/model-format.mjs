/**
 * How the reference page words what a model takes: a freeform size's bounds,
 * the vendor's lifecycle, and the limits of a param's kind. Plain JS with JSDoc
 * types, like price-format.mjs, so scripts/model-format.test.mjs runs it as is.
 */
import { decimalsOf, formatNumber } from './price-format.mjs';

/** @typedef {import('./price-format.mjs').Translate} Translate */
/** @typedef {import('../config/models.generated').SizeRange} SizeRange */
/** @typedef {import('../config/models.generated').ModelLifecycle} ModelLifecycle */
/** @typedef {import('../config/models.generated').ModelParam} ModelParam */
/** @typedef {import('../config/models.generated').AspectRatioFrames} AspectRatioFrames */

/**
 * @param {number} min
 * @param {number} max
 * @returns {string}
 */
function span(min, max) {
  return min === max ? String(min) : `${min}–${max}`;
}

/**
 * A freeform size: `256–1440 × 256–1440 px · multiples of 32`, then the pixel
 * and side-ratio limits the validator refuses a size by (L13: never snapped).
 * @param {SizeRange} r
 * @param {Translate} t
 * @param {string} locale
 * @returns {string}
 */
export function sizeRangeText(r, t, locale) {
  const parts = [t('sizeRange', { w: span(r.minWidth, r.maxWidth), h: span(r.minHeight, r.maxHeight) })];
  if (r.multipleOf !== undefined) parts.push(t('sizeMultipleOf', { n: String(r.multipleOf) }));
  if (r.minPixels !== undefined) parts.push(t('sizeMinPixels', { n: formatNumber(r.minPixels, locale) }));
  if (r.maxPixels !== undefined) parts.push(t('sizeMaxPixels', { n: formatNumber(r.maxPixels, locale) }));
  if (r.maxAspect !== undefined) parts.push(t('sizeMaxAspect', { r: formatNumber(r.maxAspect, locale) }));
  return parts.join(' · ');
}

/**
 * The vendor's lifecycle, word for word: when it deprecated the model, the
 * last day it is served, and what to use instead.
 * @param {ModelLifecycle} l
 * @param {Translate} t
 * @returns {string}
 */
export function lifecycleText(l, t) {
  const parts = [];
  if (l.deprecatedAt) parts.push(t('deprecatedOn', { date: l.deprecatedAt }));
  if (l.retiresAt) parts.push(t('retiresAfter', { date: l.retiresAt }));
  if (l.replacedBy) parts.push(t('replacedBy', { id: l.replacedBy }));
  return parts.join(' · ');
}

/**
 * A byte count in the largest decimal unit it fills: 150000000 is `150 MB`.
 * @param {number} bytes
 * @param {string} locale
 * @returns {string}
 */
export function formatBytes(bytes, locale) {
  const [unit, value] =
    bytes >= 1e9 ? ['gigabyte', bytes / 1e9] : bytes >= 1e6 ? ['megabyte', bytes / 1e6] : bytes >= 1e3 ? ['kilobyte', bytes / 1e3] : ['byte', bytes];
  return new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: 'short', maximumFractionDigits: decimalsOf(value) }).format(value);
}

/**
 * The limits of one param, as litegen enforces them: a string's length, how
 * many entries a list takes, a source mesh's size, triangles (a quad counts
 * as two) and what it must have, a palette's colours, a segmentation's
 * masks, an expand's pixels per side, a layout's one word per text box. A
 * source mesh's `formats` are the containers its vendor takes, not a limit:
 * litegen converts any other mesh it reads (proxy/model3d_source.rs
 * `prepare`).
 * @param {ModelParam} p
 * @param {Translate} t
 * @param {string} locale
 * @returns {string[]}
 */
export function paramLimitsText(p, t, locale) {
  const out = [];
  if (p.maxLength !== undefined) out.push(t('maxChars', { n: String(p.maxLength) }));
  if (p.maxItems !== undefined) out.push(t('maxItems', { n: String(p.maxItems) }));
  if (p.formats && p.formats.length > 0) out.push(t('sourceFormats', { formats: p.formats.join(', ') }));
  if (p.maxBytes !== undefined) out.push(t('maxBytes', { size: formatBytes(p.maxBytes, locale) }));
  if (p.maxFaces !== undefined) out.push(t('maxFaces', { n: formatNumber(p.maxFaces, locale) }));
  if (p.requiresSkin) out.push(t('requiresSkin'));
  if (p.requiresTexture) out.push(t('requiresTexture'));
  if (p.maxColors !== undefined) out.push(t('maxColors', { n: String(p.maxColors) }));
  if (p.forced) out.push(t('forcedPalette'));
  if (p.maxMasks !== undefined) out.push(t('maxMasks', { n: String(p.maxMasks) }));
  if (p.maxPx !== undefined) out.push(t('maxPx', { n: formatNumber(p.maxPx, locale) }));
  if (p.singleWordText) out.push(t('singleWordText'));
  return out;
}

/**
 * When a frame image sets a video's shape, as validator.rs
 * `check_frame_shape` applies it: the row's frame roles, whether a
 * `reference` image beside the frame lifts the rule, and the role an image
 * sent without one takes, when that is a frame role.
 * @param {AspectRatioFrames} f
 * @param {Translate} t
 * @param {string} locale
 * @returns {string}
 */
export function aspectRatioFramesText(f, t, locale) {
  const roles = new Intl.ListFormat(locale, { type: 'disjunction' }).format(f.roles);
  const parts = [t(f.unlessReference ? 'aspectFramesUnlessReference' : 'aspectFrames', { roles })];
  if (f.defaultRole) parts.push(t('aspectFramesDefaultRole', { role: f.defaultRole }));
  return parts.join(' · ');
}

/**
 * One clip an animate model takes: the vendor's id, its name, and the
 * canonical preset it plays (a request may name either id).
 * @param {{ id: string, label?: string, canonical?: string }} preset
 * @returns {string}
 */
export function presetText(preset) {
  return [preset.id, preset.label, preset.canonical && `(${preset.canonical})`].filter(Boolean).join(' ');
}
