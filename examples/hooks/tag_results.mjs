// A post_call hook for litegen, in Node or Bun: tag every successful
// generation answer before it goes back. For video and 3D it sees every
// status answer, including the one that carries the finished result.
//
//   hooks:
//     - path: hooks/tag_results.mjs
export function post_call(request, response, caller) {
  if (response.status < 200 || response.status >= 300) return undefined;
  // A JSON answer can be changed; a file (the converter's) keeps its bytes.
  if (response.body && typeof response.body === "object") {
    response.body.served_by = "litegen+hooks";
  }
  response.headers["x-served-by"] = "litegen+hooks";
  return response;
}
