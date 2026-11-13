// G2: every declaration the package's types export carries its doc comment,
// so an editor shows what each type, field, function and method is.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

const DTS = ['../dist/litegen_3d_converter.d.ts', '../dist/index.d.ts'];
// wasm-bindgen's own members of every exported class, which it writes
// without docs: free() and the Symbol.dispose that `using` calls.
const BINDGEN = new Set(['free', '[Symbol.dispose]']);

const documented = (node) => ts.getJSDocCommentsAndTags(node).some((d) => ts.isJSDoc(d) && (d.comment ?? '').toString().trim() !== '');

function undocumented(path) {
  const text = readFileSync(new URL(path, import.meta.url), 'utf8');
  const file = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true);
  const missing = [];
  const members = (owner, list) => {
    for (const m of list) {
      const name = m.name?.getText(file);
      if (!name || BINDGEN.has(name) || ts.isConstructorDeclaration(m)) continue;
      if (!documented(m)) missing.push(`${owner}.${name}`);
    }
  };
  for (const s of file.statements) {
    if (ts.isExportDeclaration(s) || ts.isImportDeclaration(s)) continue;
    const name = s.name?.getText(file) ?? s.declarationList?.declarations[0]?.name.getText(file);
    if (!name) continue;
    if (!documented(s)) missing.push(name);
    if (ts.isInterfaceDeclaration(s) || ts.isClassDeclaration(s)) members(name, s.members);
    if (ts.isTypeAliasDeclaration(s) && ts.isTypeLiteralNode(s.type)) members(name, s.type.members);
  }
  return missing;
}

test('every exported type, field, function and method has a doc comment', () => {
  for (const path of DTS) {
    assert.deepEqual(undocumented(path), [], path);
  }
});
