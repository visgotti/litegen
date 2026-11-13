// A request rewrite for litegen, in TypeScript: Bun runs it as it is.
// Callers may use short model aliases; litegen then validates the real model
// like any other request.
//
//   hooks:
//     - path: hooks/rewrite.ts
interface HookRequest {
  method: string;
  path: string;
  query: string;
  headers: Record<string, string>;
  body: Record<string, unknown> | null;
  files: Array<{ name: string; filename?: string; content_type?: string; size: number }>;
}

const ALIASES: Record<string, string> = {
  "fast-image": "mock/visual-image-gen",
};

export function pre_call(request: HookRequest): HookRequest | undefined {
  const model = request.body?.model;
  if (typeof model !== "string" || !(model in ALIASES)) return undefined;
  request.body!.model = ALIASES[model];
  return request;
}
