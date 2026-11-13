// README.md's ```js examples run as written: each block runs in its own Node
// process, importing the package by name, and prints exactly the lines its
// `// ` comments under each console.log show.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const readme = readFileSync(new URL('../README.md', import.meta.url), 'utf8');
const blocks = [...readme.matchAll(/```js\n([\s\S]*?)```/g)].map((m) => m[1]);
const packageDir = fileURLToPath(new URL('..', import.meta.url));

/** The comment lines that follow each console.log line: what it prints. */
function expectedOutput(code) {
  const out = [];
  let afterLog = false;
  for (const line of code.split('\n')) {
    const t = line.trim();
    if (afterLog && t.startsWith('// ')) {
      out.push(t.slice(3));
      continue;
    }
    afterLog = t.includes('console.log(');
  }
  return out;
}

test('every README example runs and prints what it says', () => {
  assert.ok(blocks.length >= 4, `${blocks.length} blocks`);
  for (const code of blocks) {
    const printed = execFileSync(process.execPath, ['--input-type=module', '-e', code], { cwd: packageDir, encoding: 'utf8' });
    assert.deepEqual(printed.trimEnd().split('\n'), expectedOutput(code), code);
  }
});
