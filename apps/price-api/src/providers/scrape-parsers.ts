import * as cheerio from 'cheerio';
import { PriceUnit } from '../common/enums';
import { Tier, tierKey } from '../common/tier-key';
import { ScrapedPriceComponent } from './types';

/**
 * Generic, provider-agnostic parsing helpers. They turn a fetched pricing page
 * (HTML) into normalised price components using a per-provider alias map. The
 * logic is deliberately resilient: it scans every table row, so it survives
 * cosmetic markup changes, and it simply omits anything it cannot confidently
 * match — the scrape service then keeps the last-known-good price for those.
 *
 * These helpers are pure functions and are unit-tested against saved fixtures,
 * so the parsing pipeline is verified without depending on a live network.
 */

/** Maps a label seen on the pricing page to one of our model components. */
export interface PriceAlias {
  modelId: string;
  /** Substring (case-insensitive) or regex identifying the model's row. */
  match: string | RegExp;
  unit: PriceUnit;
  tier?: Tier;
}

const MONEY = /\$\s*([0-9]+(?:\.[0-9]+)?)/;

function aliasMatches(alias: PriceAlias, text: string): boolean {
  return typeof alias.match === 'string'
    ? text.toLowerCase().includes(alias.match.toLowerCase())
    : alias.match.test(text);
}

/**
 * Extract price components from the `<tr>` rows of an HTML pricing page.
 * For each alias, the first row whose text matches and contains a `$amount`
 * yields a component. Aliases with no matching row are skipped.
 */
export function parsePricingTables(html: string, aliases: PriceAlias[]): ScrapedPriceComponent[] {
  const $ = cheerio.load(html);
  const out: ScrapedPriceComponent[] = [];
  const taken = new Set<string>();

  $('tr').each((_, el) => {
    const text = $(el).text().replace(/\s+/g, ' ').trim();
    if (!text) {
      return;
    }
    const money = MONEY.exec(text);
    if (!money) {
      return;
    }
    for (const alias of aliases) {
      const key = `${alias.modelId}|${alias.unit}|${tierKey(alias.tier)}`;
      if (taken.has(key)) {
        continue;
      }
      if (aliasMatches(alias, text)) {
        out.push({
          modelId: alias.modelId,
          unit: alias.unit,
          amountUsd: parseFloat(money[1]),
          tier: alias.tier ?? null,
        });
        taken.add(key);
      }
    }
  });

  return out;
}

/**
 * Map an arbitrary JSON pricing document into components. The caller supplies a
 * projection from the parsed JSON to raw `{modelId, amountUsd, ...}` records.
 */
export function parsePricingJson<T>(
  data: T,
  project: (data: T) => ScrapedPriceComponent[],
): ScrapedPriceComponent[] {
  return project(data);
}
