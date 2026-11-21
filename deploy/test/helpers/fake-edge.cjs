'use strict';

// Preloaded with `node --require` so the REAL deploy.js runs against fakes at its
// external edges only: the SSH connection, local processes (docker, npm, tar, git),
// the network and the clock. Every remote command is recorded to FAKE_EDGE_LOG.
// Nothing here can reach a server or a registry.

const Module = require('module');
const childProcess = require('child_process');
const fs = require('fs');
const net = require('net');
const path = require('path');

const LOG = process.env.FAKE_EDGE_LOG;
if (!LOG) throw new Error('fake-edge: FAKE_EDGE_LOG is required');
const scenario = JSON.parse(process.env.FAKE_EDGE_SCENARIO || '{}');

const FAKE_SHA = '0123456789abcdef0123456789abcdef01234567';
const FAKE_NOW = Date.parse('2026-01-15T12:00:00.000Z');

const record = (entry) => fs.appendFileSync(LOG, `${JSON.stringify(entry)}\n`);

function respond(cmd) {
  const ok = (stdout = '') => ({ code: 0, stdout, stderr: '' });
  // Anchored: the compose file written over SSH also mentions /health/live.
  if (/\/health\/live( >\/dev\/null)?$/.test(cmd)) return scenario.healthFails ? { code: 1, stdout: '', stderr: '' } : ok();
  if (cmd.startsWith('docker image inspect')) return ok(scenario.imageBuilt ? 'yes' : 'no');
  if (cmd.startsWith('pgrep')) return ok('no');
  if (cmd.startsWith('cat ') && cmd.includes('build.status')) return ok('0');
  return ok();
}

class FakeNodeSSH {
  async connect(cfg) { record({ type: 'connect', host: cfg.host }); }
  async execCommand(cmd) { record({ type: 'exec', cmd }); return respond(cmd); }
  async putFile(local, remote) { record({ type: 'put', local, remote }); }
  dispose() {}
}

const realLoad = Module._load;
Module._load = function load(request, ...rest) {
  if (request === 'node-ssh') return { NodeSSH: FakeNodeSSH };
  return realLoad.call(this, request, ...rest);
};

childProcess.execSync = (cmd, opts = {}) => {
  record({ type: 'local', cmd });
  if (cmd === 'git rev-parse HEAD') return `${FAKE_SHA}\n`;
  if (cmd === 'npm run build') {
    // The build output deploy.js checks for before it ships anything.
    if (path.basename(opts.cwd) === 'dashboard') {
      fs.mkdirSync(path.join(opts.cwd, 'dist'), { recursive: true });
    } else {
      const out = path.join(opts.cwd, 'out');
      fs.mkdirSync(out, { recursive: true });
      const index = path.join(out, 'en.html');
      fs.writeFileSync(index, '<html></html>');
      const fresh = new Date(FAKE_NOW + 1000);
      fs.utimesSync(index, fresh, fresh);
    }
  }
  return opts.encoding ? '' : Buffer.alloc(0);
};
childProcess.spawnSync = () => { throw new Error('fake-edge: spawnSync is not expected'); };

global.fetch = () => { throw new Error('fake-edge: network is disabled'); };
net.connect = () => { throw new Error('fake-edge: network is disabled'); };

Date.now = () => FAKE_NOW;
const realSetTimeout = global.setTimeout;
global.setTimeout = (fn, _ms, ...args) => realSetTimeout(fn, 0, ...args);

// deploy.js cleans up its local tarballs in /tmp. The faked tar never made them,
// and a test must not delete a real deploy's files.
const isLocalTarball = (p) => String(p).startsWith('/tmp/litegen-');
const realUnlink = fs.unlinkSync;
const realRm = fs.rmSync;
fs.unlinkSync = (p) => (isLocalTarball(p) ? undefined : realUnlink(p));
fs.rmSync = (p, opts) => (isLocalTarball(p) ? undefined : realRm(p, opts));
