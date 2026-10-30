// Tripo (VAST AI) — the v3 API docs, every page served as a markdown twin
// (`<page>.md`). Carried: the rig (POST /animations/rig, after the free
// POST /animations/rig-check). The vendor ids are the rig `model` versions the
// adapter sends — mapping mirrors litegen-core/src/providers/model3d/tripo.rs
// `rig_model`: v3.0 for humanoids and `others`, v2.5 for creatures. Ids come
// only from the code-formatted values opening the `### model` section's list
// items, so versions named in prose are not counted. The price page is
// server-rendered HTML; its credit lines are snapshotted for the rig family.

const DOCS = 'https://developers.tripo3d.ai/en/docs';
const VERSION = /^v\d+\.\d+-\d{8}$/;

/** A page's `### <name>` section, up to the next heading of any level. */
function section(md, name) {
  const part = md.split(/^(?=#{1,6} )/m).find((s) => s.startsWith(`### ${name}\n`));
  if (!part) throw new Error(`page has no "### ${name}" section`);
  return part;
}

/** The code-formatted values opening a section's `- ` list items. */
const listed = (s) => [...s.matchAll(/^- `([^`]+)`/gm)].map((m) => m[1]);

/** One line per parameter: its name and the values it lists (prose dropped). */
const params = (md, names) => names.map((n) => `${n}: ${listed(section(md, n)).join(', ')}`).join('\n') + '\n';

/** The price page's visible text, one cell per line. */
function priceCells(html) {
  return html
    .replace(/<script[\s\S]*?<\/script>|<style[\s\S]*?<\/style>/g, '')
    .replace(/<[^>]+>/g, '\n')
    .replace(/&amp;/g, '&')
    .replace(/&#x27;|&#39;/g, "'")
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
}

export default {
  provider: 'tripo',
  models: {
    'tripo/rig': ['v3.0-20260909', 'v2.5-20260210'],
  },
  sources: [
    {
      key: 'rig',
      url: `${DOCS}/animations-rig.md`,
      expect: 'text',
      extract: (md) => listed(section(md, 'model')).filter((id) => VERSION.test(id)),
      snapshot: (md) => params(md, ['model', 'rig_type', 'spec', 'out_format']),
    },
    {
      key: 'rig-check',
      url: `${DOCS}/animations-rig-check.md`,
      expect: 'text',
      index: true,
      extract: (md) => [...md.matchAll(/^### (\w+)$/gm)].map((m) => m[1]),
      snapshot: (md) => {
        const fields = [...md.matchAll(/^### (\w+)$/gm)].map((m) => m[1]);
        if (!fields.includes('riggable')) throw new Error('rig-check page no longer documents `riggable`');
        return `fields: ${fields.join(', ')}\nrig_type: ${listed(section(md, 'rig_type')).join(', ')}\n`;
      },
    },
    {
      key: 'pricing',
      url: 'https://developers.tripo3d.ai/en/pricing',
      expect: 'html',
      index: true,
      extract: (html) => ['Rig Check', 'Auto Rig', 'Animation Retarget'].filter((name) => priceCells(html).includes(name)),
      snapshot: (html) => {
        const cells = priceCells(html);
        const lines = [];
        const rate = cells.find((c) => /^1 credit = \$/.test(c));
        if (!rate) throw new Error('price page has no "1 credit = $…" line');
        lines.push(rate);
        for (const name of ['Rig Check', 'Auto Rig', 'Animation Retarget']) {
          const i = cells.indexOf(name);
          if (i < 0) throw new Error(`price page has no "${name}" row`);
          lines.push(`${name}: ${cells[i + 1]}`);
        }
        return lines.join('\n') + '\n';
      },
    },
  ],
  acknowledged: [
    { id: 'v1.0-20240301', reason: 'skipped 2026-10-30: biped-only legacy rig model (Tripo\'s default); v3.0-20260909 is "recommended for humanoid characters" and takes every spec' },
  ],
  untyped: {},
};
