import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/**
 * Contract test: the hand-written `LiteGenClient` facade must surface a method
 * for every operation in the OpenAPI spec that `litegen-core` emits — and must
 * never call a path that is neither in the spec nor a known-undocumented core
 * endpoint.
 *
 * How it works (purely static — no network, no running server):
 *   1. Parse `sdks/openapi.json` into a set of `METHOD /normalized/path`.
 *   2. Scan `src/client.ts` for every `request("METHOD", "/path")` call and
 *      normalize the same way (strip query string, collapse `${id}`/`{id}` → `{}`).
 *   3. Diff the two sets, honoring two ratchets in `sdks/contract-allowlist.json`:
 *        - known_uncovered.typescript — spec ops with no method yet
 *        - known_undocumented.paths   — facade calls for core endpoints the spec omits
 *
 * The ratchets keep the suite green today but fail the moment a NEW spec
 * operation ships without a method, the facade calls an undeclared/undocumented
 * path, or a ratchet entry becomes covered/stale (forcing the list to shrink).
 */

const here = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));

const SPEC_PATH = here("../../openapi.json");
const FACADE_PATH = here("../src/client.ts");
const ALLOWLIST_PATH = here("../../contract-allowlist.json");

const HTTP_METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/** Collapse path params + drop query string so spec and facade paths compare. */
function normalizePath(path: string): string {
  return path
    .split("?")[0]
    .replace(/\$\{[^}]+\}/g, "{}") // TS template params: ${encodeURIComponent(id)}
    .replace(/\{[^}]+\}/g, "{}"); // OpenAPI params: {id}
}

function specOperations(): Set<string> {
  const spec = JSON.parse(readFileSync(SPEC_PATH, "utf8")) as {
    paths?: Record<string, Record<string, unknown>>;
  };
  const ops = new Set<string>();
  for (const [path, item] of Object.entries(spec.paths ?? {})) {
    for (const method of Object.keys(item)) {
      if (HTTP_METHODS.includes(method.toUpperCase())) {
        ops.add(`${method.toUpperCase()} ${normalizePath(path)}`);
      }
    }
  }
  return ops;
}

function facadeOperations(): Set<string> {
  const src = readFileSync(FACADE_PATH, "utf8");
  // Matches: request("POST", "/v1/...")  and  request<T>("GET", `/v1/.../${id}`).
  // The leading-slash requirement excludes method arrays like ["GET","HEAD"].
  const re = /"(GET|POST|PUT|PATCH|DELETE)"\s*,\s*[`"](\/[^`"]+)[`"]/g;
  const ops = new Set<string>();
  let m: RegExpExecArray | null;
  while ((m = re.exec(src)) !== null) {
    ops.add(`${m[1]} ${normalizePath(m[2])}`);
  }
  return ops;
}

const allow = JSON.parse(readFileSync(ALLOWLIST_PATH, "utf8")) as {
  known_uncovered?: { typescript?: string[] };
  known_undocumented?: { paths?: string[] };
};
const knownUncovered = new Set<string>(allow.known_uncovered?.typescript ?? []);
const knownUndocumented = new Set<string>(allow.known_undocumented?.paths ?? []);

const spec = specOperations();
const facade = facadeOperations();

describe("LiteGenClient ↔ OpenAPI contract", () => {
  it("exposes a method for every spec operation (minus known gaps)", () => {
    const uncovered = [...spec]
      .filter((op) => !facade.has(op) && !knownUncovered.has(op))
      .sort();
    // Non-empty ⇒ an endpoint shipped without an SDK method. Add the method, or
    // (if intentionally unsupported) add it to known_uncovered.typescript.
    expect(uncovered.join("\n")).toBe("");
  });

  it("never calls a path that is neither in the spec nor known-undocumented", () => {
    const drift = [...facade]
      .filter((op) => !spec.has(op) && !knownUndocumented.has(op))
      .sort();
    // Non-empty ⇒ the facade calls a path the spec doesn't declare and that
    // isn't a recorded core-spec gap — likely a typo or a renamed endpoint.
    expect(drift.join("\n")).toBe("");
  });

  it("keeps known_uncovered honest (no now-covered or stale entries)", () => {
    const nowCovered = [...knownUncovered].filter((op) => facade.has(op)).sort();
    expect(nowCovered.join("\n")).toBe(""); // covered now → remove from allowlist
    const stale = [...knownUncovered].filter((op) => !spec.has(op)).sort();
    expect(stale.join("\n")).toBe(""); // not in spec → remove from allowlist
  });

  it("keeps known_undocumented honest (entries are actually used + still absent from spec)", () => {
    const nowDocumented = [...knownUndocumented].filter((op) => spec.has(op)).sort();
    expect(nowDocumented.join("\n")).toBe(""); // spec now documents it → remove
    const unused = [...knownUndocumented].filter((op) => !facade.has(op)).sort();
    expect(unused.join("\n")).toBe(""); // facade no longer calls it → remove
  });

  it("parsed a non-trivial spec (guards against an empty/locator bug)", () => {
    expect(spec.size).toBeGreaterThanOrEqual(25);
    expect(facade.size).toBeGreaterThanOrEqual(25);
  });
});
