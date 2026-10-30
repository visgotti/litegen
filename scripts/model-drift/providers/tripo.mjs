// Tripo (VAST AI) — the v3 API docs, every page served as a markdown twin
// (`<page>.md`). Carried: the rig (POST /animations/rig, after the free
// POST /animations/rig-check) and the retarget (POST /animations/retarget).
// The rig's vendor ids are the rig `model` versions the adapter sends —
// mapping mirrors litegen-core/src/providers/model3d/tripo.rs `rig_model`:
// v3.0 for humanoids and `others`, v2.5 for creatures — read only from the
// code-formatted values opening the `### model` section's list items. The
// retarget's are the preset ids the catalog row offers, read from the
// `### animation` section's bullets for v2.5/v3.0 rigs (the v1.0 rig's
// `preset:biped:*` list is acknowledged). Image-to-multiview
// (POST /generation/image-to-multiview, the image adapter
// litegen-core/src/providers/image/tripo.rs) has no model versions: its
// vendor id is the endpoint's name, and its snapshot the request field and the
// view URLs the result names. The price page is server-rendered HTML; its
// credit lines are snapshotted.

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
    'tripo/animate': [
      'preset:idle', 'preset:walk', 'preset:run', 'preset:dive', 'preset:climb', 'preset:jump',
      'preset:slash', 'preset:shoot', 'preset:hurt', 'preset:fall', 'preset:turn',
      'preset:quadruped:walk', 'preset:hexapod:walk', 'preset:octopod:walk',
      'preset:serpentine:march', 'preset:aquatic:march',
    ],
    'tripo/multiview': 'image-to-multiview',
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
      key: 'retarget',
      url: `${DOCS}/animations-retarget.md`,
      expect: 'text',
      // Every code-formatted preset id in the `### animation` section: the
      // v2.5/v3.0 bullets and the v1.0 run-on list.
      extract: (md) => [...section(md, 'animation').matchAll(/`(preset:[\w:.-]+)`/g)].map((m) => m[1]),
      snapshot: (md) => {
        const animation = section(md, 'animation');
        const v3 = [...animation.matchAll(/^- [^:\n]+: (.+)$/gm)].map((m) => m[1].trim()).join('\n');
        return `presets (v2.5/v3.0):\n${v3}\n` + params(md, ['out_format']) + `animate_in_place: ${/### animate_in_place/.test(md)}\n`;
      },
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
      key: 'image-to-multiview',
      url: `${DOCS}/generation-image-to-multiview.md`,
      expect: 'text',
      extract: (md) => [...md.matchAll(/^\*\*Endpoint:\*\* `POST \/generation\/([\w-]+)`/gm)].map((m) => m[1]),
      snapshot: (md) => {
        // Request and response fields: the `###` headings before the examples.
        const fields = [...md.split(/^## Request Example/m)[0].matchAll(/^### (\S+)$/gm)].map((m) => m[1]);
        const views = [...new Set([...md.matchAll(/\b(\w+_view_url)\b/g)].map((m) => m[1]))];
        if (!views.length) throw new Error('multiview page names no *_view_url');
        const endpoints = [...md.matchAll(/^\*\*Endpoint:\*\* `POST \/generation\/([\w-]+)`/gm)].map((m) => m[1]);
        return `endpoint: ${endpoints.join(', ')}\nfields: ${fields.join(', ')}\nviews: ${views.join(', ')}\n`;
      },
    },
    {
      key: 'pricing',
      url: 'https://developers.tripo3d.ai/en/pricing',
      expect: 'html',
      index: true,
      extract: (html) => ['Rig Check', 'Auto Rig', 'Animation Retarget', 'Image to Multiview'].filter((name) => priceCells(html).includes(name)),
      snapshot: (html) => {
        const cells = priceCells(html);
        const lines = [];
        const rate = cells.find((c) => /^1 credit = \$/.test(c));
        if (!rate) throw new Error('price page has no "1 credit = $…" line');
        lines.push(rate);
        for (const name of ['Rig Check', 'Auto Rig', 'Animation Retarget', 'Image to Multiview']) {
          const i = cells.indexOf(name);
          if (i < 0) throw new Error(`price page has no "${name}" row`);
          lines.push(`${name}: ${cells[i + 1]}`);
        }
        return lines.join('\n') + '\n';
      },
    },
  ],
  acknowledged: [
    { pattern: /^preset:biped:/, reason: 'skipped 2026-10-30: the v1.0 rig model\'s 90+ biped presets; tripo/rig makes v2.5/v3.0 rigs, which take the preset: ids carried' },
    { id: 'v1.0-20240301', reason: 'skipped 2026-10-30: biped-only legacy rig model (Tripo\'s default); v3.0-20260909 is "recommended for humanoid characters" and takes every spec' },
  ],
  untyped: {},
};
