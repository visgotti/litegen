/**
 * A price tier is an optional set of qualifiers (e.g. `{resolution: "1080p",
 * quality: "hd"}`) that distinguishes otherwise-identical price components for
 * the same model+unit. To enforce uniqueness in the database we derive a stable
 * canonical string key from the tier object: keys sorted, JSON-serialised, with
 * `*` representing "no tier".
 */
export type Tier = Record<string, string> | null | undefined;

export function tierKey(tier: Tier): string {
  if (!tier || Object.keys(tier).length === 0) {
    return '*';
  }
  const sorted = Object.keys(tier)
    .sort()
    .reduce<Record<string, string>>((acc, k) => {
      acc[k] = tier[k];
      return acc;
    }, {});
  return JSON.stringify(sorted);
}
