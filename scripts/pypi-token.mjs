#!/usr/bin/env node
// One-shot helper: opens PyPI in a real browser, waits for YOU to log in
// (handle password + 2FA yourself), then creates an "Entire account" API token
// and saves it to /tmp/pypi-token.txt (and prints it).
//
// Run from the repo root:   node scripts/pypi-token.mjs
//
// It's resilient: if PyPI's form markup has changed and the auto-fill misses,
// just finish creating the token in the window — the script polls the page for
// the generated `pypi-…` value and captures it either way.

import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { writeFileSync } from 'node:fs';

// Resolve Playwright from the dashboard's node_modules (it's installed there).
const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(resolve(here, '../dashboard/package.json'));
const { chromium } = require('@playwright/test');

const TOKEN_NAME = 'litegen-sdk-publish';
const OUT = '/tmp/pypi-token.txt';

const browser = await chromium.launch({ headless: false, slowMo: 60 });
const page = await (await browser.newContext()).newPage();

console.log('\n=== PyPI API token helper ===');
console.log('1) A browser window is opening at PyPI.');
console.log('2) LOG IN yourself (email/username + password + 2FA). Create an account first if you don\'t have one.');
console.log('3) I\'ll then fill + submit the token form ("Entire account" scope) and capture the token.\n');

await page.goto('https://pypi.org/manage/account/token/', { waitUntil: 'domcontentloaded' });

// The token-name field only renders once you're authenticated — wait up to 5 min.
console.log('⏳ Waiting for you to log in (up to 5 minutes)...');
const nameInput = page.locator('input#description, input[name="description"]');
try {
  await nameInput.waitFor({ state: 'visible', timeout: 300000 });
  console.log('✓ Logged in — filling the token form...');
  try { await nameInput.fill(TOKEN_NAME); } catch { console.log('  (couldn\'t fill the name — type one in the window)'); }
  // Scope: "Entire account (all projects)" → <select> value "scope:user".
  try {
    await page.selectOption('select#token_scope, select[name="token_scope"]', 'scope:user');
  } catch {
    console.log('  (couldn\'t auto-select scope — choose "Entire account (all projects)" in the window)');
  }
  try {
    await page.locator('button:has-text("Create token"), input[type=submit][value*="Create"]').first().click({ timeout: 5000 });
  } catch { console.log('  (couldn\'t click Create — click "Create token" in the window)'); }
} catch {
  console.log('⚠ Didn\'t detect the token form in time — if you\'re logged in, create the token manually in the window.');
}

// Poll the page for the generated token (works whether auto-filled or done by hand) — up to 4 min.
console.log('⏳ Waiting for the token to appear...');
let token = null;
for (let i = 0; i < 120; i++) {
  let body = '';
  try { body = await page.locator('body').innerText(); } catch { /* navigations */ }
  const m = body.match(/pypi-[A-Za-z0-9_\-]{30,}/);
  if (m) { token = m[0]; break; }
  await page.waitForTimeout(2000);
}

if (token) {
  writeFileSync(OUT, token + '\n', { mode: 0o600 });
  console.log('\n✅ TOKEN CREATED — saved to ' + OUT);
  console.log('PYPI_TOKEN=' + token);
  console.log('\nLeave it with me — I\'ll publish litegen-sdk and you can revoke this token afterward.');
} else {
  console.log('\n⚠ Could not capture a token automatically. Copy the pypi-… value from the window and paste it to me.');
}

await page.waitForTimeout(6000);
await browser.close();
