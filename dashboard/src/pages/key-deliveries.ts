// The Keys page's webhook deliveries panel, kept free of React so it is
// unit-tested. The panel lists attempts, newest first; a redeliver acts on a
// delivery (`delivery_id`, the X-Litegen-Delivery-Id its receiver got).

/** The rows that carry a Redeliver button: the newest attempt of each
 *  delivery. An attempt recorded before migration 029 has no delivery id. */
export function redeliverRowIds(rows: ReadonlyArray<{ id: string; delivery_id: string | null }>): Set<string> {
  const seen = new Set<string>();
  const ids = new Set<string>();
  for (const row of rows) {
    if (!row.delivery_id || seen.has(row.delivery_id)) continue;
    seen.add(row.delivery_id);
    ids.add(row.id);
  }
  return ids;
}
