// Read a glTF or GLB with @gltf-transform/core — a reader independent of
// litegen's — and print what a consumer of the file sees, as JSON: each
// animation (name, channel count, last key time), the skins' joint names and
// the extensions the file requires. The PixelGenie §3J chain test
// (litegen-core/tests/model3d_chain.rs) runs it on the chain's final GLB.
//
//   node read-gltf.mjs <file.glb>
import { NodeIO } from '@gltf-transform/core';

const [file] = process.argv.slice(2);
if (!file) {
  console.error('usage: node read-gltf.mjs <file.glb>');
  process.exit(2);
}
const root = (await new NodeIO().read(file)).getRoot();
const lastKey = (sampler) => sampler.getInput()?.getMax([])[0] ?? 0;
console.log(JSON.stringify({
  animations: root.listAnimations().map((a) => ({
    name: a.getName(),
    channels: a.listChannels().length,
    duration_s: Math.max(0, ...a.listSamplers().map(lastKey)),
  })),
  joints: root.listSkins().flatMap((s) => s.listJoints().map((j) => j.getName())),
  extensionsRequired: root.listExtensionsRequired().map((e) => e.extensionName),
}));
