/**
 * Tiny, dependency-free OpenAPI 3.1 reader used by the in-page REST reference.
 *
 * We deliberately avoid pulling in a heavy spec library (or a third-party doc
 * widget like Redoc): the renderer only needs a normalized, render-ready view
 * of the spec, so this module flattens `$ref`/`allOf`, derives human type
 * labels, and synthesizes request/response examples. Everything here is pure —
 * the component memoizes `buildApiModel(spec)` once at module/render time.
 */

/** The subset of JSON Schema we actually read. OpenAPI 3.1 schemas are a JSON
 *  Schema dialect, so nullability shows up as `type: ['string', 'null']`. */
export interface JsonSchema {
  $ref?: string;
  type?: string | string[];
  format?: string;
  title?: string;
  description?: string;
  enum?: unknown[];
  const?: unknown;
  default?: unknown;
  example?: unknown;
  examples?: unknown[];
  properties?: Record<string, JsonSchema>;
  required?: string[];
  items?: JsonSchema;
  allOf?: JsonSchema[];
  oneOf?: JsonSchema[];
  anyOf?: JsonSchema[];
  additionalProperties?: boolean | JsonSchema;
  nullable?: boolean;
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  minItems?: number;
  maxItems?: number;
  pattern?: string;
}

interface ParameterObject {
  $ref?: string;
  name?: string;
  in?: 'path' | 'query' | 'header' | 'cookie';
  required?: boolean;
  description?: string;
  schema?: JsonSchema;
}

interface MediaType {
  schema?: JsonSchema;
}

interface OperationObject {
  tags?: string[];
  summary?: string;
  description?: string;
  operationId?: string;
  deprecated?: boolean;
  parameters?: ParameterObject[];
  requestBody?: { description?: string; required?: boolean; content?: Record<string, MediaType> };
  responses?: Record<string, { description?: string; content?: Record<string, MediaType> }>;
}

type PathItem = { parameters?: ParameterObject[] } & Record<string, unknown>;

export interface OpenApiSpec {
  openapi?: string;
  info?: { title?: string; version?: string; description?: string; license?: { name?: string } };
  tags?: { name: string; description?: string }[];
  paths?: Record<string, PathItem>;
  components?: { schemas?: Record<string, JsonSchema>; parameters?: Record<string, ParameterObject> };
}

/* ── Render-ready model ──────────────────────────────────────────────────── */

export type HttpMethod = 'get' | 'post' | 'put' | 'patch' | 'delete' | 'head' | 'options';

export interface Field {
  name: string;
  required: boolean;
  /** Human-readable type, e.g. `string`, `RefRole[]`, `string | null`. */
  type: string;
  description?: string;
  enumValues?: string[];
  /** Compact constraint string, e.g. `≥ 0`, `1–64 chars`. */
  constraint?: string;
  default?: string;
  nullable?: boolean;
  /** Nested object / array-of-object fields, depth-capped. */
  fields?: Field[];
}

export interface Param {
  name: string;
  in: string;
  required: boolean;
  type: string;
  description?: string;
  constraint?: string;
}

export interface ResponseView {
  status: string;
  description?: string;
  fields?: Field[];
  /** A pretty-printed JSON example, when a schema is present. */
  example?: string;
  schemaName?: string;
}

export interface Endpoint {
  id: string;
  method: HttpMethod;
  path: string;
  /** Clean human title (the `METHOD /path — ` prefix is stripped). */
  title: string;
  description?: string;
  deprecated: boolean;
  params: Param[];
  /** Flattened request-body fields (application/json), if any. */
  bodyFields?: Field[];
  bodySchemaName?: string;
  bodyRequired: boolean;
  /** A ready-to-run curl invocation. */
  curl: string;
  responses: ResponseView[];
}

export interface TagGroup {
  name: string;
  description?: string;
  endpoints: Endpoint[];
}

export interface ApiModel {
  title: string;
  version: string;
  description?: string;
  license?: string;
  groups: TagGroup[];
  count: number;
}

const METHOD_ORDER: HttpMethod[] = ['get', 'post', 'put', 'patch', 'delete', 'head', 'options'];
const MAX_DEPTH = 4;
/** curl base — mirrors the placeholder host used across the SDK snippets. */
const CURL_BASE = 'https://your-litegen-host';

/* ── $ref + composition resolution ──────────────────────────────────────── */

function refName(ref: string): string {
  return ref.slice(ref.lastIndexOf('/') + 1);
}

function lookupSchema(ref: string, spec: OpenApiSpec): JsonSchema | undefined {
  if (!ref.startsWith('#/components/schemas/')) return undefined;
  return spec.components?.schemas?.[refName(ref)];
}

function lookupParam(ref: string, spec: OpenApiSpec): ParameterObject | undefined {
  if (!ref.startsWith('#/components/parameters/')) return undefined;
  return spec.components?.parameters?.[refName(ref)];
}

/** Follow a single `$ref` hop. Returns the schema plus its component name (used
 *  as a type label) when the ref pointed at `#/components/schemas/*`. */
function resolve(schema: JsonSchema, spec: OpenApiSpec): { schema: JsonSchema; name?: string } {
  let name: string | undefined;
  let current = schema;
  // Chase chains of $ref defensively (rare, but cheap to support).
  for (let i = 0; current.$ref && i < 8; i++) {
    name = refName(current.$ref);
    const next = lookupSchema(current.$ref, spec);
    if (!next) break;
    current = next;
  }
  return { schema: current, name };
}

/** Drop the JSON-Schema `null` member; report whether it was present. */
function splitNullable(type: string | string[] | undefined): { types: string[]; nullable: boolean } {
  if (type === undefined) return { types: [], nullable: false };
  const arr = Array.isArray(type) ? type : [type];
  const nullable = arr.includes('null');
  return { types: arr.filter((t) => t !== 'null'), nullable };
}

/** Human-readable type label for a schema (resolves one $ref hop for naming). */
export function typeLabel(raw: JsonSchema, spec: OpenApiSpec): string {
  const { schema, name } = resolve(raw, spec);
  if (name && (schema.type === 'object' || schema.properties || schema.allOf)) return name;

  if (schema.allOf?.length) return name ?? 'object';
  const variants = schema.oneOf ?? schema.anyOf;
  if (variants?.length) {
    const labels = variants.map((v) => typeLabel(v, spec));
    return [...new Set(labels)].join(' | ') || 'object';
  }

  const { types, nullable } = splitNullable(schema.type);
  let base: string;
  if (types.includes('array')) {
    base = schema.items ? `${typeLabel(schema.items, spec)}[]` : 'array';
  } else if (types.length) {
    base = types[0];
  } else if (schema.properties) {
    base = name ?? 'object';
  } else if (schema.enum) {
    base = 'enum';
  } else {
    base = 'any';
  }
  return nullable ? `${base} | null` : base;
}

function constraintOf(s: JsonSchema): string | undefined {
  const parts: string[] = [];
  if (s.minimum !== undefined && s.maximum !== undefined) parts.push(`${s.minimum}–${s.maximum}`);
  else if (s.minimum !== undefined) parts.push(`≥ ${s.minimum}`);
  else if (s.maximum !== undefined) parts.push(`≤ ${s.maximum}`);
  if (s.minLength !== undefined && s.maxLength !== undefined) parts.push(`${s.minLength}–${s.maxLength} chars`);
  else if (s.maxLength !== undefined) parts.push(`≤ ${s.maxLength} chars`);
  else if (s.minLength !== undefined) parts.push(`≥ ${s.minLength} chars`);
  if (s.format) parts.push(s.format);
  return parts.length ? parts.join(' · ') : undefined;
}

function enumValues(s: JsonSchema): string[] | undefined {
  if (!s.enum?.length) return undefined;
  return s.enum.map((v) => (typeof v === 'string' ? v : JSON.stringify(v)));
}

/** Merge `allOf` members + own properties into a single property bag. */
function collectProperties(
  raw: JsonSchema,
  spec: OpenApiSpec,
): { properties: Record<string, JsonSchema>; required: Set<string> } {
  const properties: Record<string, JsonSchema> = {};
  const required = new Set<string>();
  const absorb = (schema: JsonSchema) => {
    const { schema: r } = resolve(schema, spec);
    for (const part of r.allOf ?? []) absorb(part);
    if (r.properties) Object.assign(properties, r.properties);
    for (const req of r.required ?? []) required.add(req);
  };
  absorb(raw);
  return { properties, required };
}

/** Flatten a schema into render-ready {@link Field}s (recursing into nested
 *  objects / arrays of objects up to {@link MAX_DEPTH}). */
export function flattenFields(raw: JsonSchema, spec: OpenApiSpec, depth = 0): Field[] {
  if (depth >= MAX_DEPTH) return [];
  const { schema } = resolve(raw, spec);
  const { properties, required } = collectProperties(schema, spec);
  const names = Object.keys(properties);
  if (names.length === 0) return [];

  return names.map((name) => {
    const propRaw = properties[name];
    const { schema: prop } = resolve(propRaw, spec);
    const { nullable } = splitNullable(prop.type);
    const field: Field = {
      name,
      required: required.has(name),
      type: typeLabel(propRaw, spec),
      description: prop.description ?? propRaw.description,
      enumValues: enumValues(prop),
      constraint: constraintOf(prop),
      default: prop.default !== undefined ? JSON.stringify(prop.default) : undefined,
      nullable,
    };

    // Recurse into object properties and arrays-of-objects.
    const { types } = splitNullable(prop.type);
    if (prop.properties || prop.allOf) {
      const children = flattenFields(prop, spec, depth + 1);
      if (children.length) field.fields = children;
    } else if (types.includes('array') && prop.items) {
      const { schema: items } = resolve(prop.items, spec);
      if (items.properties || items.allOf) {
        const children = flattenFields(items, spec, depth + 1);
        if (children.length) field.fields = children;
      }
    }
    return field;
  });
}

/* ── Example synthesis ──────────────────────────────────────────────────── */

function exampleForPrimitive(s: JsonSchema): unknown {
  if (s.example !== undefined) return s.example;
  if (s.default !== undefined) return s.default;
  if (s.enum?.length) return s.enum[0];
  if (s.const !== undefined) return s.const;
  const { types } = splitNullable(s.type);
  const t = types[0];
  switch (t) {
    case 'integer':
    case 'number':
      return 0;
    case 'boolean':
      return true;
    case 'string':
      if (s.format === 'date-time') return '2026-01-01T00:00:00Z';
      if (s.format === 'uuid') return '00000000-0000-0000-0000-000000000000';
      if (s.format === 'email') return 'user@example.com';
      if (s.format === 'uri' || s.format === 'url') return 'https://…';
      return 'string';
    default:
      return null;
  }
}

/** Build a representative JS value for a schema (depth- and cycle-guarded). */
export function buildExample(raw: JsonSchema, spec: OpenApiSpec, depth = 0, seen = new Set<string>()): unknown {
  const { schema, name } = resolve(raw, spec);
  if (schema.example !== undefined) return schema.example;
  if (name && seen.has(name)) return {};
  if (name) seen = new Set(seen).add(name);
  if (depth >= MAX_DEPTH) return schema.properties || schema.allOf ? {} : exampleForPrimitive(schema);

  const variants = schema.oneOf ?? schema.anyOf;
  if (variants?.length) return buildExample(variants[0], spec, depth, seen);

  const { properties } = collectProperties(schema, spec);
  if (Object.keys(properties).length) {
    const out: Record<string, unknown> = {};
    for (const [key, prop] of Object.entries(properties)) {
      out[key] = buildExample(prop, spec, depth + 1, seen);
    }
    return out;
  }

  const { types } = splitNullable(schema.type);
  if (types.includes('array')) {
    return schema.items ? [buildExample(schema.items, spec, depth + 1, seen)] : [];
  }
  return exampleForPrimitive(schema);
}

/* ── curl synthesis ─────────────────────────────────────────────────────── */

function curlFor(method: HttpMethod, path: string, bodyExample: unknown): string {
  const url = `${CURL_BASE}${path}`;
  const lines = [`curl -X ${method.toUpperCase()} ${url} \\`, `  -H "Authorization: Bearer $LITEGEN_KEY"`];
  if (bodyExample !== undefined) {
    lines[lines.length - 1] += ' \\';
    lines.push('  -H "Content-Type: application/json" \\');
    const json = JSON.stringify(bodyExample, null, 2)
      .split('\n')
      .map((l, i) => (i === 0 ? l : `  ${l}`))
      .join('\n');
    lines.push(`  -d '${json}'`);
  }
  return lines.join('\n');
}

/* ── Top-level assembly ─────────────────────────────────────────────────── */

const SUMMARY_PREFIX = /^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+\/\S*\s*[—–-]\s*/i;

function cleanTitle(summary: string | undefined, method: HttpMethod, path: string): string {
  if (!summary) return `${method.toUpperCase()} ${path}`;
  const stripped = summary.replace(SUMMARY_PREFIX, '').trim();
  return stripped || summary;
}

function jsonMedia(content: Record<string, MediaType> | undefined): MediaType | undefined {
  if (!content) return undefined;
  return content['application/json'] ?? content[Object.keys(content)[0]];
}

function buildParams(
  pathItem: PathItem,
  op: OperationObject,
  spec: OpenApiSpec,
): Param[] {
  const raw = [...(pathItem.parameters ?? []), ...(op.parameters ?? [])];
  return raw
    .map((p) => (p.$ref ? lookupParam(p.$ref, spec) ?? p : p))
    .filter((p): p is ParameterObject => Boolean(p?.name && p.in !== 'header' && p.in !== 'cookie'))
    .map((p) => {
      const schema = p.schema ?? {};
      return {
        name: p.name!,
        in: p.in ?? 'query',
        required: Boolean(p.required) || p.in === 'path',
        type: typeLabel(schema, spec),
        description: p.description,
        constraint: constraintOf(schema),
      };
    });
}

function buildResponses(op: OperationObject, spec: OpenApiSpec): ResponseView[] {
  const responses = op.responses ?? {};
  return Object.keys(responses)
    .sort((a, b) => Number(a) - Number(b))
    .map((status) => {
      const r = responses[status];
      const media = jsonMedia(r.content);
      const view: ResponseView = { status, description: r.description };
      if (media?.schema) {
        const { name } = resolve(media.schema, spec);
        view.schemaName = name;
        const fields = flattenFields(media.schema, spec);
        if (fields.length) view.fields = fields;
        // Only synthesize example bodies for success responses to keep the UI calm.
        if (status.startsWith('2')) view.example = JSON.stringify(buildExample(media.schema, spec), null, 2);
      }
      return view;
    });
}

/** Normalize a raw OpenAPI document into the render-ready {@link ApiModel}. */
export function buildApiModel(spec: OpenApiSpec): ApiModel {
  const tagMeta = new Map((spec.tags ?? []).map((t) => [t.name, t.description]));
  const tagOrder = (spec.tags ?? []).map((t) => t.name);
  const buckets = new Map<string, Endpoint[]>();
  let count = 0;

  for (const [path, item] of Object.entries(spec.paths ?? {})) {
    for (const method of METHOD_ORDER) {
      const op = item[method] as OperationObject | undefined;
      if (!op || typeof op !== 'object') continue;
      count++;

      const bodyMedia = jsonMedia(op.requestBody?.content);
      let bodyFields: Field[] | undefined;
      let bodySchemaName: string | undefined;
      let bodyExample: unknown;
      if (bodyMedia?.schema) {
        const { name } = resolve(bodyMedia.schema, spec);
        bodySchemaName = name;
        const fields = flattenFields(bodyMedia.schema, spec);
        if (fields.length) bodyFields = fields;
        bodyExample = buildExample(bodyMedia.schema, spec);
      }

      const endpoint: Endpoint = {
        id: op.operationId ?? `${method}-${path}`.replace(/[^\w]+/g, '-').replace(/^-|-$/g, ''),
        method,
        path,
        title: cleanTitle(op.summary, method, path),
        description: op.description,
        deprecated: Boolean(op.deprecated),
        params: buildParams(item, op, spec),
        bodyFields,
        bodySchemaName,
        bodyRequired: Boolean(op.requestBody?.required),
        curl: curlFor(method, path, bodyExample),
        responses: buildResponses(op, spec),
      };

      const tag = op.tags?.[0] ?? 'Other';
      const arr = buckets.get(tag) ?? [];
      arr.push(endpoint);
      buckets.set(tag, arr);
    }
  }

  // Order groups by the spec's declared tag order, with any extras appended.
  const orderedNames = [
    ...tagOrder.filter((t) => buckets.has(t)),
    ...[...buckets.keys()].filter((t) => !tagOrder.includes(t)),
  ];
  const groups: TagGroup[] = orderedNames.map((name) => ({
    name,
    description: tagMeta.get(name),
    endpoints: buckets.get(name)!,
  }));

  return {
    title: spec.info?.title ?? 'API',
    version: spec.info?.version ?? '',
    description: spec.info?.description,
    license: spec.info?.license?.name,
    groups,
    count,
  };
}
