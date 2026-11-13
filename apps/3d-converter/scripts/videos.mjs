// Turns the e2e project's videos (test-results/**/video.webm) into MP4s
// named after their tests, in e2e-artifacts/videos/, and joins them in run
// order into all-conversions.mp4. Needs ffmpeg with libx264.
import { execFileSync } from 'node:child_process';
import { mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const results = path.join(root, 'test-results');
const out = path.join(root, 'e2e-artifacts/videos');
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

const found = readdirSync(results)
  .map((dir) => ({ dir, video: path.join(results, dir, 'video.webm') }))
  .filter(({ video }) => {
    try {
      return statSync(video).isFile();
    } catch {
      return false;
    }
  })
  // The functional tests last; the matrix in sample order (directory names
  // carry the test titles).
  .sort((a, b) => Number(a.dir.includes('functional')) - Number(b.dir.includes('functional')) || statSync(a.video).mtimeMs - statSync(b.video).mtimeMs);

const mp4s = [];
for (const { dir, video } of found) {
  const name = `${dir.replace(/-e2e$/, '').replace(/[^a-z0-9-]+/gi, '-').slice(0, 80)}.mp4`;
  const target = path.join(out, name);
  execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', video, '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'veryfast', '-crf', '23', '-movflags', '+faststart', target]);
  mp4s.push(target);
}
const list = path.join(out, 'concat.txt');
writeFileSync(list, mp4s.map((p) => `file '${p.replace(/'/g, "'\\''")}'`).join('\n'));
const all = path.join(out, 'all-conversions.mp4');
execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-f', 'concat', '-safe', '0', '-i', list, '-c', 'copy', '-movflags', '+faststart', all]);
rmSync(list);
console.log([...mp4s, all].join('\n'));
