// Custom auth for litegen, in Node or Bun: callers send your token, the hook
// injects a litegen key. The same contract as custom_auth.py:
//   - `x-acme-token: <token>` instead of a litegen API key;
//   - ACME_TOKENS maps tokens to users (`token:user,token:user`);
//   - a known token gets LITEGEN_APP_KEY and is recorded as that end user;
//   - a request that already carries a litegen key is left to litegen.
//
//   hooks:
//     - path: hooks/custom_auth.mjs
const TOKENS = new Map(
  (process.env.ACME_TOKENS ?? "")
    .split(",")
    .filter((pair) => pair.includes(":"))
    .map((pair) => {
      const [token, ...user] = pair.trim().split(":");
      return [token, user.join(":")];
    }),
);

export function pre_call(request) {
  const { headers } = request;
  if (headers.authorization) return undefined; // already a litegen caller
  const user = TOKENS.get(headers["x-acme-token"] ?? "");
  if (!user) {
    // An error with a status refuses the request with that status.
    const refusal = new Error("send a valid x-acme-token");
    refusal.status = 401;
    refusal.headers = { "www-authenticate": "AcmeToken" };
    throw refusal;
  }
  headers.authorization = `Bearer ${process.env.LITEGEN_APP_KEY}`;
  headers["x-litegen-end-user"] = user;
  delete headers["x-acme-token"]; // litegen doesn't need it
  return request;
}
