/**
 * Model-comparison data for the landing page.
 *
 * Each comparison is one static evaluation prompt run across several
 * provider:model pairs, so visitors can eyeball how the models stack up on the
 * *same* prompt. The <ModelComparison> section renders a pill per prompt, then a
 * grid of the result images with a `provider:model` caption under each.
 *
 * ── Adding a comparison ──────────────────────────────────────────────────────
 *   1. Drop the result images in `public/comparisons/<id>/` (one per model).
 *      Square-ish images look best; any aspect ratio works (they're contained).
 *   2. Add an entry below. `image` is the path *under* public/comparisons/.
 *
 * The whole section is HIDDEN automatically while COMPARISONS is empty, so it's
 * safe to ship with none and fill it in later.
 *
 * Example (copy, uncomment, swap in real files):
 *
 *   {
 *     id: 'red-panda',
 *     label: 'Red panda',                                   // short pill text
 *     prompt: 'a red panda coding at a desk, cinematic lighting',
 *     results: [
 *       { provider: 'openai',    model: 'dall-e-3', image: 'red-panda/openai.png' },
 *       { provider: 'stability', model: 'sdxl',     image: 'red-panda/stability.png' },
 *       { provider: 'google',    model: 'imagen-3', image: 'red-panda/google.png' },
 *     ],
 *   },
 */

/** One model's output for a comparison prompt. */
export interface ComparisonResult {
  /** Provider id, shown before the colon (e.g. "openai"). */
  provider: string;
  /** Model id, shown after the colon (e.g. "dall-e-3"). */
  model: string;
  /** Image path under public/comparisons/ (e.g. "red-panda/openai.png"). */
  image: string;
}

/** One evaluation prompt run across several models. */
export interface Comparison {
  /** Stable slug — also the conventional image subfolder name. */
  id: string;
  /** Short label for the selector pill. Falls back to `prompt` if omitted. */
  label?: string;
  /** The exact prompt sent to every model, shown above the grid. */
  prompt: string;
  /** One entry per model; rendered left-to-right in this order. */
  results: ComparisonResult[];
}

/**
 * Fill this in to surface the comparison panel. Empty → the section is hidden.
 */
export const COMPARISONS: Comparison[] = [];
