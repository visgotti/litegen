// Lays every screenshot of the matrix run (e2e-artifacts/shots) out on one
// labelled page, one row per sample and one column per target, and saves a
// picture of it: e2e-artifacts/contact-sheet.png. Run after the e2e project.
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const art = path.join(root, 'e2e-artifacts');
const rows = readFileSync(path.join(art, 'report.jsonl'), 'utf8').trim().split('\n').map((l) => JSON.parse(l));
const samples = [...new Set(rows.map((r) => r.sample))];
const columns = [...new Set(rows.map((r) => `${r.to}|${r.packaging}`))];
const cell = (r) => `<figure><img src="shots/${r.sample}-to-${r.to}-${r.packaging}.png"><figcaption>${r.to}${r.packaging === 'files' ? ' (files)' : ''} · iou ${r.iou.toFixed(2)} · ${r.losses} lost</figcaption></figure>`;
const html = `<!doctype html><meta charset="utf-8"><style>
body{margin:16px;background:#0d1117;color:#e1e4e8;font:12px -apple-system,sans-serif}
h1{font-size:16px;margin:0 0 4px} p{color:#8b949e;margin:0 0 12px}
table{border-collapse:collapse} td,th{padding:3px;vertical-align:top}
th{color:#8b949e;font-weight:500;text-align:left;white-space:nowrap}
figure{margin:0;width:150px} img{width:150px;height:auto;display:block;border:1px solid #30363d;border-radius:4px}
figcaption{color:#8b949e;font-size:10px;margin-top:2px}
</style><h1>litegen 3D Converter: every sample to every format, drawn by three.js's own loader for it</h1>
<p>${rows.length} conversions in the browser · ${samples.length} samples · columns are targets, rows are sources (first column: the input as it was read)</p>
<table><tr><th></th><th>input</th>${columns.map((c) => `<th>${c.replace('|file', '').replace('|files', ' (files)')}</th>`).join('')}</tr>
${samples.map((s) => `<tr><th>${s}</th><td><figure><img src="shots/${s}-input.png"><figcaption>input</figcaption></figure></td>${columns.map((c) => { const r = rows.find((x) => x.sample === s && `${x.to}|${x.packaging}` === c); return `<td>${r ? cell(r) : ''}</td>`; }).join('')}</tr>`).join('\n')}
</table>`;
const page_ = path.join(art, 'contact-sheet.html');
writeFileSync(page_, html);
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 2600, height: 1200 } });
await page.goto(pathToFileURL(page_).href);
await page.waitForLoadState('networkidle');
await page.screenshot({ path: path.join(art, 'contact-sheet.png'), fullPage: true });
await browser.close();
console.log(`${path.join(art, 'contact-sheet.png')}: ${rows.length} conversions`);
