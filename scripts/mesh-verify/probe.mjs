#!/usr/bin/env node
// Load Collada files with three.js r183's own ColladaLoader, in the same
// headless Chrome the harness renders with, and print what the loader built:
// materials (colour after its colour management, side, opacity, texture wrap),
// geometry attributes (with the first colour values), skeletons and clips. It
// is how the DAE codec's claims about three.js are checked against the loader
// itself rather than against a reading of its source.
//
//   node probe.mjs <dir> <file.dae> [<file.dae> ...]     (paths relative to <dir>)

import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';

const here = path.dirname(fileURLToPath(import.meta.url));
const [dir, ...files] = process.argv.slice(2);
if (!dir || !files.length) {
  console.error('usage: node probe.mjs <dir> <file.dae> [...]');
  process.exit(2);
}
const roots = { '/page/': path.join(here, 'page'), '/three/': path.join(here, 'node_modules', 'three'), '/f/': path.resolve(dir) };
const server = http.createServer(async (req, res) => {
  const url = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  for (const [prefix, root] of Object.entries(roots)) {
    if (!url.startsWith(prefix)) continue;
    const file = path.join(root, url.slice(prefix.length));
    try {
      const body = await fs.readFile(file);
      const type = file.endsWith('.js') ? 'text/javascript' : file.endsWith('.html') ? 'text/html' : file.endsWith('.png') ? 'image/png' : 'application/octet-stream';
      res.writeHead(200, { 'content-type': type });
      res.end(body);
    } catch {
      res.writeHead(404);
      res.end();
    }
    return;
  }
  res.writeHead(404);
  res.end();
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();
const logs = [];
page.on('console', (m) => logs.push(`${m.type()}: ${m.text()}`));
await page.goto(`${base}/page/index.html`);
await page.waitForFunction(() => window.ready === true, null, { timeout: 30000 });

for (const f of files) {
  logs.length = 0;
  const out = await page.evaluate(async (url) => {
    const THREE = await import('three');
    const { ColladaLoader } = await import('three/addons/loaders/ColladaLoader.js');
    let result;
    try {
      result = await new ColladaLoader().loadAsync(url);
    } catch (e) {
      return { error: String(e && e.stack ? e.stack.split('\n').slice(0, 3).join(' | ') : e) };
    }
    if (!result) return { error: 'loader returned null' };
    // Let TextureLoader requests settle so wrap modes and images are final.
    await new Promise((r) => setTimeout(r, 300));
    const wrap = (w) => ({ [THREE.RepeatWrapping]: 'repeat', [THREE.ClampToEdgeWrapping]: 'clamp', [THREE.MirroredRepeatWrapping]: 'mirror' })[w] || w;
    const mats = new Map();
    const meshes = [];
    result.scene.traverse((o) => {
      if (!o.isMesh) return;
      const g = o.geometry;
      const attrs = {};
      for (const [k, a] of Object.entries(g.attributes)) attrs[k] = `${a.count}x${a.itemSize}`;
      const color = g.attributes.color ? Array.from(g.attributes.color.array.slice(0, 8)).map((v) => +v.toFixed(4)) : null;
      meshes.push({ name: o.name, type: o.type, attrs, firstColors: color, bones: o.skeleton ? o.skeleton.bones.map((b) => b.name) : null });
      for (const m of Array.isArray(o.material) ? o.material : [o.material]) {
        if (mats.has(m.uuid)) continue;
        mats.set(m.uuid, {
          name: m.name,
          type: m.type,
          color: m.color ? [m.color.r, m.color.g, m.color.b].map((v) => +v.toFixed(4)) : null,
          emissive: m.emissive ? [m.emissive.r, m.emissive.g, m.emissive.b].map((v) => +v.toFixed(4)) : null,
          side: m.side === THREE.DoubleSide ? 'double' : m.side === THREE.BackSide ? 'back' : 'front',
          opacity: m.opacity,
          transparent: m.transparent,
          shininess: m.shininess,
          map: m.map ? { wrapS: wrap(m.map.wrapS), wrapT: wrap(m.map.wrapT), src: m.map.image && m.map.image.src ? m.map.image.src.split('/').slice(-2).join('/') : null } : null,
        });
      }
    });
    const clips = (result.scene.animations || []).map((c) => ({
      name: c.name,
      duration: +c.duration.toFixed(4),
      tracks: c.tracks.map((t) => `${t.name.split('.').pop()}:${t.times.length}:${t.getInterpolation() === THREE.InterpolateDiscrete ? 'STEP' : t.getInterpolation() === THREE.InterpolateLinear ? 'LINEAR' : t.getInterpolation()}`),
    }));
    return { materials: [...mats.values()], meshes, clips };
  }, `${base}/f/${f}`);
  console.log(JSON.stringify({ file: f, ...out, warnings: logs.filter((l) => /warn|error/i.test(l)).slice(0, 6) }, null, 1));
}
await browser.close();
server.close();
