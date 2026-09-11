// Shared extractors for provider definitions (scripts/model-drift/providers/*.mjs).
//
// OpenAPI helpers read *request* schemas of the endpoints our adapter calls,
// so a model that only appears in, say, a usage-report enum is not mistaken
// for something the endpoint accepts. Excerpts are the typings evidence a
// weekly review diffs: structural keys only (prose churns weekly), keys
// sorted, discriminated unions trimmed to the branches for models we carry.

const PROSE_KEYS = new Set(['description', 'example', 'examples', 'title', 'summary', 'externalDocs', '$schema']);
const COMBINATORS = ['oneOf', 'anyOf', 'allOf'];

function resolveRef(doc, ref) {
  if (!ref.startsWith('#/')) throw new Error(`external $ref not supported: ${ref}`);
  return ref
    .slice(2)
    .split('/')
    .reduce((node, seg) => node?.[seg.replace(/~1/g, '/').replace(/~0/g, '~')], doc);
}

/** Inline local $refs. A ref already being expanded is left as-is (recursive schemas). */
function deref(doc, node, stack = new Set()) {
  if (Array.isArray(node)) return node.map((n) => deref(doc, n, stack));
  if (!node || typeof node !== 'object') return node;
  if (typeof node.$ref === 'string') {
    if (stack.has(node.$ref)) return { $ref: node.$ref };
    const target = resolveRef(doc, node.$ref);
    if (target === undefined) throw new Error(`unresolvable $ref ${node.$ref}`);
    const { $ref, ...siblings } = node;
    return deref(doc, { ...target, ...siblings }, new Set(stack).add($ref));
  }
  return Object.fromEntries(Object.entries(node).map(([k, v]) => [k, deref(doc, v, stack)]));
}

function requestSchemas(doc, path, method) {
  const op = doc.paths?.[path]?.[method];
  if (!op) throw new Error(`endpoint ${method.toUpperCase()} ${path} is not in the spec — renamed or removed upstream`);
  const content = deref(doc, op.requestBody ?? {}).content ?? {};
  return Object.entries(content).map(([contentType, c]) => ({ contentType, schema: c.schema ?? {} }));
}

/** const/enum values of the `model` property, through combinators on both the schema and the property. */
function modelValues(schema) {
  const out = [];
  const visitModel = (m) => {
    if (!m || typeof m !== 'object') return;
    if (m.const !== undefined) out.push(m.const);
    if (Array.isArray(m.enum)) out.push(...m.enum);
    for (const k of COMBINATORS) for (const b of m[k] ?? []) visitModel(b);
  };
  const visit = (s) => {
    if (!s || typeof s !== 'object') return;
    if (s.properties?.model) visitModel(s.properties.model);
    for (const k of COMBINATORS) for (const b of s[k] ?? []) visit(b);
  };
  visit(schema);
  return out.filter((v) => typeof v === 'string');
}

/** Model ids the named endpoints accept (request body `model` const/enum). */
export function openapiModelIds(doc, { paths, method = 'post' }) {
  return [...new Set(paths.flatMap((p) => requestSchemas(doc, p, method).flatMap(({ schema }) => modelValues(schema))))];
}

/** Drop prose/vendor-extension keywords, but never a property *named* "title" etc. */
function prune(node, isPropertiesMap = false) {
  if (Array.isArray(node)) return node.map((n) => prune(n));
  if (!node || typeof node !== 'object') return node;
  const out = {};
  for (const [k, v] of Object.entries(node)) {
    if (!isPropertiesMap && (PROSE_KEYS.has(k) || k.startsWith('x-'))) continue;
    out[k] = prune(v, !isPropertiesMap && k === 'properties');
  }
  return out;
}

function sortKeys(v) {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v && typeof v === 'object') return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sortKeys(v[k])]));
  return v;
}

export const stableStringify = (v) => JSON.stringify(sortKeys(v), null, 2) + '\n';

/**
 * Typings excerpt for the named endpoints: each request schema, pruned; when
 * the schema is a union discriminated by `model`, only the branches for the
 * vendor ids we carry (`ours`), so another vendor's new model does not make
 * our snapshot churn — that surfaces as a new id instead.
 */
export function openapiExcerpt(doc, { paths, ours, method = 'post' }) {
  const want = new Set(ours.map((s) => s.toLowerCase()));
  const carried = (b) => modelValues(b).some((v) => want.has(v.toLowerCase()));
  const out = {};
  for (const path of [...paths].sort()) {
    for (const { contentType, schema } of requestSchemas(doc, path, method)) {
      const branches = schema.oneOf ?? schema.anyOf;
      const discriminated = Array.isArray(branches) && branches.length > 0 && branches.every((b) => modelValues(b).length > 0);
      const kept = discriminated
        ? { oneOf: branches.filter(carried).sort((a, b) => modelValues(a)[0].localeCompare(modelValues(b)[0])) }
        : schema;
      out[`${method.toUpperCase()} ${path} (${contentType})`] = prune(kept);
    }
  }
  return stableStringify(out);
}

/**
 * Fenced code blocks of a markdown docs page (request/response specs), without
 * the prose around them. Fences close only on a run of backticks at least as
 * long as the opener (CommonMark), because Mintlify wraps whole OpenAPI specs
 * in ````yaml blocks that contain ``` lines of their own.
 */
export function markdownExcerpt(md, { lang } = {}) {
  const blocks = [];
  for (const m of md.matchAll(/^(`{3,})([^\n`]*)\n([\s\S]*?)^\1`*[ \t]*$/gm)) {
    if (lang && m[2].trim().split(/\s+/)[0] !== lang) continue;
    blocks.push(m[3].replace(/\s+$/, ''));
  }
  return blocks.join('\n\n') + '\n';
}

/**
 * Line-based pruning of prose keys from block-style YAML (no YAML dependency):
 * a `description:` / `example:` / … key is dropped with every deeper-indented
 * line after it (folded and literal scalars included) — unless its parent is
 * `properties`, where the key is a request parameter's *name*.
 */
export function yamlPrune(text) {
  const out = [];
  const stack = [];
  let skipDeeperThan = null;
  for (const line of text.split('\n')) {
    if (line.trim() === '') {
      if (skipDeeperThan === null) out.push(line);
      continue;
    }
    const indent = line.match(/^ */)[0].length;
    if (skipDeeperThan !== null) {
      if (indent > skipDeeperThan) continue;
      skipDeeperThan = null;
    }
    while (stack.length && stack[stack.length - 1].indent >= indent) stack.pop();
    const key = line.match(/^ *(?:- )?([A-Za-z0-9_$-]+):(?:\s|$)/)?.[1];
    if (key) {
      if (PROSE_KEYS.has(key) && stack[stack.length - 1]?.key !== 'properties') {
        skipDeeperThan = indent;
        continue;
      }
      stack.push({ indent, key });
    }
    out.push(line);
  }
  return out.join('\n').replace(/\s+$/, '') + '\n';
}

/**
 * Strict block-style YAML subset, for vendors that publish only a YAML spec
 * (no dependency). Maps, sequences, plain/quoted scalars and block scalars
 * (prose only in practice, kept whole); anything else — flow collections,
 * anchors, tags — throws instead of being guessed at, so a spec that starts
 * using it fails its source loudly.
 */
export function parseYaml(text) {
  const lines = [];
  text.split('\n').forEach((raw, n) => {
    const indent = raw.search(/\S/);
    if (indent < 0 || raw[indent] === '#' || /^(---|\.\.\.)\s*$/.test(raw)) return;
    lines.push({ n: n + 1, indent, text: raw.slice(indent).trimEnd() });
  });
  let i = 0;
  const err = (why) => new Error(`YAML line ${lines[Math.min(i, lines.length - 1)]?.n}: ${why}`);
  const KEY = /^("(?:[^"\\]|\\.)*"|'(?:[^']|'')*'|[^\s'"#-][^#]*?|-[^\s#][^#]*?)\s*:(?:\s+(.*))?$/;
  const isItem = (t) => t === '-' || t.startsWith('- ');
  const unquote = (s) => (s.startsWith('"') ? JSON.parse(s) : s.startsWith("'") ? s.slice(1, -1).replace(/''/g, "'") : s);

  const scalar = (s) => {
    if (s.startsWith('"')) return JSON.parse(s);
    if (s.startsWith("'")) {
      if (!/^'(?:[^']|'')*'$/.test(s)) throw err(`bad single-quoted scalar ${s.slice(0, 40)}`);
      return unquote(s);
    }
    s = s.replace(/\s+#.*$/, '');
    if (s === '[]') return [];
    if (s === '{}') return {};
    if (/^[[{&*!%@`|>]/.test(s)) throw err(`unsupported YAML syntax: ${s.slice(0, 40)}`);
    if (s === 'true' || s === 'false') return s === 'true';
    if (s === 'null' || s === '~') return null;
    if (/^-?\d+(\.\d+)?$/.test(s)) return Number(s);
    return s;
  };
  const deeper = (indent) => {
    const out = [];
    while (i < lines.length && lines[i].indent > indent) out.push(lines[i++].text);
    return out;
  };
  // What follows "key:" or "-": an inline scalar (possibly continued on deeper
  // lines, or a block scalar — prose only in practice), or a nested node.
  const value = (inline, indent, afterKey) => {
    if (inline) return /^[|>][-+0-9]*$/.test(inline) ? deeper(indent).join('\n') : scalar([inline, ...deeper(indent)].join(' '));
    if (i < lines.length && lines[i].indent > indent) return node(lines[i].indent);
    if (afterKey && i < lines.length && lines[i].indent === indent && isItem(lines[i].text)) return seq(indent);
    return null;
  };
  const map = (indent) => {
    const out = {};
    while (i < lines.length && lines[i].indent === indent) {
      const m = lines[i].text.match(KEY);
      if (!m || isItem(lines[i].text)) break;
      i++;
      out[unquote(m[1])] = value(m[2], indent, true);
    }
    return out;
  };
  const seq = (indent) => {
    const out = [];
    while (i < lines.length && lines[i].indent === indent && isItem(lines[i].text)) {
      const rest = lines[i].text.slice(1).trimStart();
      if (isItem(rest) || KEY.test(rest)) {
        // "- key: value" / "- - x": the item's node starts where `rest` does
        const col = indent + lines[i].text.length - rest.length;
        lines[i] = { ...lines[i], indent: col, text: rest };
        out.push(node(col));
      } else {
        i++;
        out.push(value(rest, indent, false));
      }
    }
    return out;
  };
  const node = (indent) => {
    if (isItem(lines[i].text)) return seq(indent);
    if (KEY.test(lines[i].text)) return map(indent);
    const parts = [];
    while (i < lines.length && lines[i].indent >= indent) parts.push(lines[i++].text);
    return scalar(parts.join(' '));
  };

  if (lines.length === 0) throw new Error('empty YAML document');
  const doc = node(lines[0].indent);
  if (i < lines.length) throw err(`unexpected line: ${lines[i].text.slice(0, 40)}`);
  return doc;
}
