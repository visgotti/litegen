'use strict';

// A stand-in for the docker CLI, so the generated shell scripts run for real under
// sh and dash. It models the daemon rules those scripts depend on, including that
// `docker image rm <tag>` on a multi-tagged image only untags it, even when a
// container is using that image.

const fs = require('fs');

const STATE = process.env.FAKE_DOCKER_STATE;
const state = JSON.parse(fs.readFileSync(STATE, 'utf8'));
const save = () => fs.writeFileSync(STATE, JSON.stringify(state));
const args = process.argv.slice(2);
state.calls.push(args);

const repoOf = (ref) => ref.replace(/@.*$/, '').replace(/:[^/:]*$/, '');
const refsOf = (img) => [...img.tags, ...img.digests];
const byId = (id) => state.images.find((img) => img.id === id);
const fail = (msg) => { save(); process.stderr.write(`${msg}\n`); process.exit(1); };

function imageRm(refs) {
  let failed = false;
  for (const ref of refs) {
    const img = state.images.find((i) => refsOf(i).includes(ref));
    if (!img) { failed = true; continue; }
    const used = state.containers.some((c) => c.image === img.id);
    if (used && refsOf(img).length === 1) { failed = true; continue; }
    img.tags = img.tags.filter((t) => t !== ref);
    img.digests = img.digests.filter((d) => d !== ref);
    if (refsOf(img).length === 0) state.images = state.images.filter((i) => i !== img);
  }
  return failed;
}

function main() {
  if (args[0] === 'ps' && args[1] === '-aq') {
    return state.containers.map((c) => `${c.id}\n`).join('');
  }
  if (args[0] === 'inspect' && args[1] === '--format' && args[2] === '{{.Image}}') {
    return args.slice(3).map((id) => `${state.containers.find((c) => c.id === id).image}\n`).join('');
  }
  if (args[0] === 'image' && args[1] === 'ls') {
    const repo = args[args.length - 1];
    return state.images
      .flatMap((img) => img.tags.filter((t) => repoOf(t) === repo).map(() => `${img.id}\n`))
      .join('');
  }
  if (args[0] === 'image' && args[1] === 'inspect' && args[3] === '{{.Id}} {{.Created}}') {
    return args.slice(4).map((id) => `${id} ${byId(id).created}\n`).join('');
  }
  if (args[0] === 'image' && args[1] === 'inspect' && args[3].startsWith('{{range .RepoTags}}')) {
    return refsOf(byId(args[4])).map((ref) => `${ref}\n`).join('');
  }
  if (args[0] === 'image' && args[1] === 'rm') {
    if (imageRm(args.slice(2))) fail('conflict: unable to remove reference');
    return '';
  }
  if (args[0] === 'exec' && args[1] === 'edge-caddy-1' && args[2] === 'caddy') {
    const verb = args[3];
    // What the edge would read at this moment, through the real import path.
    state.seen.push({ verb, siteFile: fs.readFileSync(process.env.FAKE_SITE_FILE, 'utf8') });
    if (state.failOn === verb) fail(`caddy ${verb}: failed`);
    return '';
  }
  return fail(`fake docker: unexpected command: ${args.join(' ')}`);
}

const out = main();
save();
process.stdout.write(out);
