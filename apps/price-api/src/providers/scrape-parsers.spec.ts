import { PriceUnit } from '../common/enums';
import { PriceAlias, parsePricingTables } from './scrape-parsers';

const FIXTURE = `
<html><body>
  <h1>Pricing</h1>
  <table>
    <tr><th>Model</th><th>Price</th></tr>
    <tr><td>DALL·E 3</td><td>$0.04 / image</td></tr>
    <tr><td>DALL·E 2</td><td>$0.02 / image</td></tr>
    <tr><td>Unrelated model</td><td>$9.99</td></tr>
  </table>
</body></html>
`;

describe('parsePricingTables', () => {
  const aliases: PriceAlias[] = [
    { modelId: 'openai/dall-e-3', match: /dall.?e[\s·-]*3/i, unit: PriceUnit.PER_IMAGE },
    { modelId: 'openai/dall-e-2', match: /dall.?e[\s·-]*2/i, unit: PriceUnit.PER_IMAGE },
  ];

  it('extracts the right price for each matched alias', () => {
    const result = parsePricingTables(FIXTURE, aliases);
    expect(result).toContainEqual({
      modelId: 'openai/dall-e-3',
      unit: PriceUnit.PER_IMAGE,
      amountUsd: 0.04,
      tier: null,
    });
    expect(result).toContainEqual({
      modelId: 'openai/dall-e-2',
      unit: PriceUnit.PER_IMAGE,
      amountUsd: 0.02,
      tier: null,
    });
  });

  it('ignores rows that match no alias', () => {
    const result = parsePricingTables(FIXTURE, aliases);
    expect(result).toHaveLength(2);
  });

  it('returns nothing when the page structure yields no matches (degrades gracefully)', () => {
    expect(parsePricingTables('<html><body>no tables here</body></html>', aliases)).toEqual([]);
  });
});
