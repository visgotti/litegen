/**
 * How the reference page prints a model's price. Plain JS with JSDoc types, so
 * `node --test` (scripts/price-format.test.mjs) runs it as is and TypeScript
 * reads its types through `allowJs`.
 *
 * Amounts are never rounded: a catalog price such as 0.003 or 0.2113536 is
 * printed with every digit it has (and at least 2 for money).
 */

/** @type {Map<string, Intl.NumberFormat>} */
const formats = new Map();

/**
 * @param {string} locale
 * @param {Intl.NumberFormatOptions} options
 * @returns {Intl.NumberFormat}
 */
function numberFormat(locale, options) {
  const key = `${locale}|${JSON.stringify(options)}`;
  let f = formats.get(key);
  if (!f) {
    f = new Intl.NumberFormat(locale, options);
    formats.set(key, f);
  }
  return f;
}

/**
 * How many decimals a number really has: its digits to 12 places, trailing
 * zeros dropped, so binary noise past that (0.1 * 3) is not shown as a digit.
 * @param {number} x
 * @returns {number}
 */
export function decimalsOf(x) {
  const fraction = Math.abs(x).toFixed(12).split('.')[1] ?? '';
  return fraction.replace(/0+$/, '').length;
}

/**
 * A USD amount for the page, in the locale's currency format, with every
 * decimal it has and at least 2: 0.003 is `$0.003`, 7.5 is `$7.50` (es:
 * `0,003 US$`, `7,50 US$`).
 * @param {number} usd
 * @param {string} locale
 * @returns {string}
 */
export function formatUsd(usd, locale) {
  return numberFormat(locale, {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: 2,
    maximumFractionDigits: Math.max(2, decimalsOf(usd)),
  }).format(usd);
}

/**
 * A plain number (a factor, an anchor, a range bound, a limit) in the locale's
 * format, with every decimal it has: 7.111617 stays 7.111617, where a default
 * Intl format would print 7.112.
 * @param {number} x
 * @param {string} locale
 * @returns {string}
 */
export function formatNumber(x, locale) {
  return numberFormat(locale, { maximumFractionDigits: decimalsOf(x) }).format(x);
}

/**
 * next-intl's translator for the `reference` messages
 * (`useTranslations('reference')`), typed loosely so its `t` fits.
 * @typedef {(key: string, values?: Record<string, string>) => string} Translate
 */

/** @typedef {import('../config/models.generated').PriceCondition} PriceCondition */
/** @typedef {import('../config/models.generated').PriceEffect} PriceEffect */
/** @typedef {import('../config/models.generated').PriceRuleEntry} PriceRuleEntry */
/** @typedef {import('../config/models.generated').ModelPricing} ModelPricing */

/** Operators for a range: one bound alone (`x ≥ 1`), or the bounds around the fact (`1 < x ≤ 4`). */
const ALONE = { gt: '>', gte: '≥', lt: '<', lte: '≤' };
const BELOW_FACT = { gt: '<', gte: '≤' };
const ABOVE_FACT = { lt: '<', lte: '≤' };

/**
 * One condition: `resolution = 540p`, `references sent`, `references ≥ 1`,
 * `1.048576 < megapixels ≤ 4.194304`. Fact names and values stay verbatim;
 * the operators are symbols, kept out of the messages (an ICU message with an
 * angle bracket does not parse).
 * @param {PriceCondition} c
 * @param {Translate} t
 * @param {string} locale
 * @returns {string}
 */
export function conditionText(c, t, locale) {
  if (c.is === 'sent') return t('ruleSent', { fact: c.fact });
  if (c.is === 'eq') return `${c.fact} = ${typeof c.value === 'number' ? formatNumber(c.value, locale) : String(c.value)}`;
  if (c.is === 'range') {
    /** @type {(b: 'gt' | 'gte' | 'lt' | 'lte') => string} */
    const n = (b) => formatNumber(/** @type {number} */ (c[b]), locale);
    const lower = /** @type {const} */ (['gt', 'gte']).filter((b) => c[b] !== undefined);
    const upper = /** @type {const} */ (['lt', 'lte']).filter((b) => c[b] !== undefined);
    if (lower.length === 1 && upper.length === 1) {
      return `${n(lower[0])} ${BELOW_FACT[lower[0]]} ${c.fact} ${ABOVE_FACT[upper[0]]} ${n(upper[0])}`;
    }
    return [...lower, ...upper].map((b) => `${c.fact} ${ALONE[b]} ${n(b)}`).join(` ${t('ruleAnd')} `);
  }
  throw new Error(`a price condition the page cannot word: ${JSON.stringify(c)}`);
}

/**
 * A rule's one effect: a price (`$0.02`), a rate (`$1.50 per second`), or an
 * adjustment (`× 2`, `+ $0.15`, `− $0.01`).
 * @param {PriceEffect} e
 * @param {Translate} t
 * @param {string} locale
 * @returns {string}
 */
export function effectText(e, t, locale) {
  const price = formatUsd(Math.abs(e.value), locale);
  switch (e.kind) {
    case 'usd':
      return t('effectUsd', { price: formatUsd(e.value, locale) });
    case 'perSecond':
      return t('effectPerSecond', { price: formatUsd(e.value, locale) });
    case 'perMegapixel':
      return t('effectPerMegapixel', { price: formatUsd(e.value, locale) });
    case 'perItem':
      return t('effectPerItem', { price: formatUsd(e.value, locale), list: e.list });
    case 'multiply':
      return t('effectMultiply', { k: formatNumber(e.value, locale) });
    case 'multiplyPerMegapixelAbove':
      return t('effectMultiplyPerMegapixelAbove', { mp: formatNumber(e.value, locale) });
    case 'addUsd':
      // The loader allows a negative add_usd (vidu's q2-turbo lowers its price).
      return e.value < 0 ? t('effectSubtractUsd', { price }) : t('effectAddUsd', { price });
    default:
      throw new Error(`a price effect the page cannot word: ${JSON.stringify(e)}`);
  }
}

/**
 * One rule as the engine applies it: `resolution = 540p: $0.75 per second`.
 * @param {PriceRuleEntry} rule
 * @param {Translate} t
 * @param {string} locale
 * @returns {string}
 */
export function ruleText(rule, t, locale) {
  const when =
    rule.when.length === 0 ? t('ruleAlways') : rule.when.map((c) => conditionText(c, t, locale)).join(` ${t('ruleAnd')} `);
  return t('rule', { when, effect: effectText(rule.effect, t, locale) });
}

/**
 * A row head's price: `$0.003 / image`, `7,50 US$ / vídeo`.
 * @param {ModelPricing} pricing
 * @param {Translate} t
 * @param {string} locale
 * @returns {string}
 */
export function priceText(pricing, t, locale) {
  return t('pricePer', { price: formatUsd(pricing.baseCostUsd, locale), unit: t(`priceUnit.${pricing.unit}`) });
}

/**
 * What the head price is for, in the conformance test's own terms
 * (litegen-core tests/catalog_conformance.rs,
 * every_base_price_is_what_the_rules_charge_for_the_default_request): every
 * param at its default, plus one clip on an animate model and keyframes at
 * 0 s and 5 s on a keyframe model.
 * @param {{ pricing?: ModelPricing, params: { name: string }[], refRoles: { name: string }[] }} m
 * @param {Translate} t
 * @param {string} locale
 * @returns {string}
 */
export function priceDefaultText(m, t, locale) {
  if (!m.pricing) throw new Error('a model with no price has no default request price');
  const values = { price: formatUsd(m.pricing.baseCostUsd, locale), unit: t(`priceUnit.${m.pricing.unit}`) };
  const clip = m.params.some((p) => p.name === 'animations');
  const keyframes = m.refRoles.some((r) => r.name === 'keyframe');
  if (clip && keyframes) throw new Error('no wording for a default request with both a clip and keyframes');
  if (clip) return t('priceDefaultOneClip', values);
  if (keyframes) return t('priceDefaultKeyframes', values);
  return t('priceDefault', values);
}
