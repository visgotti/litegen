// Every sample (one per source format) converted to every format the
// package lists, in the browser, through the page; each result drawn by
// three.js's own loader for its format. Then the same for every format that
// writes sidecars, as separate files. One test per sample, so one video each.
import { appendFileSync, mkdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { convert, formats } from '@litegen/3d-converter';
import { drawn, iou, openApp, written } from './helpers';

/** `reference` names the conversion a static sample's shape is compared
 *  with when its format's three.js loader draws it incompletely. */
interface Sample { id: string; title: string; format: string; static: boolean; files: string[]; reference?: string }
const samples = JSON.parse(readFileSync(new URL('../public/samples/samples.json', import.meta.url), 'utf8')) as Sample[];
const targets = formats();
const shots = fileURLToPath(new URL('../e2e-artifacts/shots/', import.meta.url));
const report = fileURLToPath(new URL('../e2e-artifacts/report.jsonl', import.meta.url));
mkdirSync(shots, { recursive: true });

/** The floor for a static model's silhouette overlap, input against output. */
const MIN_IOU = 0.98;

const sampleFiles = (s: Sample) =>
  Object.fromEntries(s.files.map((name) => [name, new Uint8Array(readFileSync(new URL(`../public/samples/${s.id}/${name}`, import.meta.url)))]));



for (const sample of samples) {
  test(`${sample.format} ${sample.id}: converts to every format`, async ({ page }) => {
    test.setTimeout(15 * 60_000);
    const errors = await openApp(page);
    await page.getByTestId(`sample-${sample.id}`).click();
    const input = await drawn(page.getByTestId('input-viewer'), sample.format);
    if (sample.reference) {
      // The app says so when the format's loader drew less than the converter reads.
      await expect(page.getByTestId('input-drawn-note')).toBeVisible();
    }
    await page.getByTestId('input').screenshot({ path: `${shots}${sample.id}-input.png` });
    const output = page.getByTestId('output-viewer');
    // What the shapes are compared with: the input as its loader drew it, or
    // the reference conversion once it is drawn.
    let reference = sample.reference ? null : input.silhouette;

    const check = async (to: string, packaging: 'file' | 'files') => {
      await written(page, to, packaging);
      const got = await drawn(output, to);
      await expect(page.getByTestId('error')).toHaveCount(0);
      expect(got.coverage, `${sample.id} to ${to} (${packaging}) draws something`).toBeGreaterThanOrEqual(0.005);
      if (reference === null && to === sample.reference && packaging === 'file') reference = got.silhouette;
      const overlap = iou(reference ?? input.silhouette, got.silhouette);
      // Every sample, animated ones included: silhouettes are taken at rest.
      expect(overlap, `${sample.id} to ${to} (${packaging}): the same shape`).toBeGreaterThanOrEqual(MIN_IOU);
      // A clip keeps the model in its frame: halfway through, it covers at
      // least a third of what it covers at rest (a misread rotation throws
      // it out of frame).
      const mid = await output.getAttribute('data-clip-coverage');
      // A target that keeps animation keeps an animated sample's clip ...
      const keeps = targets.find((f) => f.format === to)!;
      if (!sample.static && (packaging === 'files' ? keeps.keepsInPackage : keeps.keeps).animation) {
        expect(mid, `${sample.id} to ${to} (${packaging}): the clip survived`).not.toBeNull();
      }
      // ... and the clip keeps the model in its frame.
      if (mid !== null) expect(Number(mid), `${sample.id} to ${to} (${packaging}): mid-clip coverage`).toBeGreaterThanOrEqual(got.coverage / 3);
      const files = await page.getByTestId('output-files').locator('tbody tr').count();
      const losses = Number(await page.getByTestId('output-losses').getAttribute('data-count'));
      const took = (await page.getByTestId('took').textContent()) ?? '';
      await output.screenshot({ path: `${shots}${sample.id}-to-${to}-${packaging}.png` });
      appendFileSync(report, `${JSON.stringify({ sample: sample.id, from: sample.format, to, packaging, files, losses, coverage: got.coverage, iou: overlap, took })}\n`);
      await page.waitForTimeout(600);
    };

    for (const t of targets) {
      await page.getByTestId(`target-${t.format}`).click();
      if (t.writesSidecars) await page.getByTestId('package-file').click();
      await check(t.format, 'file');
    }
    for (const t of targets.filter((f) => f.writesSidecars)) {
      await page.getByTestId(`target-${t.format}`).click();
      await page.getByTestId('package-files').click();
      await check(t.format, 'files');
      // Every file the package writes is listed: the model, and as many
      // sidecars as this model needs (none for an OBJ of a cube with no
      // materials).
      const want = convert(sampleFiles(sample), { to: t.format, package: 'files' }).files.length;
      await expect(page.getByTestId('output-files').locator('tbody tr')).toHaveCount(want);
    }
    expect(errors, errors.join('\n')).toEqual([]);
  });
}
