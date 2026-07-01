// Shared test helpers: a recording fake `fetch` so the runners/client can be
// driven without any network or live server.

/**
 * Build a fake fetch that records every call and replies from a route table.
 *
 * @param {Record<string, (req:{method:string,url:string,path:string,headers:object,body:any})=>{status?:number,json?:any,text?:string,bytes?:Uint8Array|number[]}>} routes
 *   keyed by "METHOD /api/path" (path matched WITHOUT the base URL). The handler
 *   returns { status?, json?, text?, bytes? }. `bytes` drives the binary
 *   (`arrayBuffer`) path for artifact downloads. A missing route → 404.
 * @returns {{ fetch: typeof fetch, calls: Array<object> }}
 */
export function makeFakeFetch(routes = {}) {
  const calls = [];
  async function fakeFetch(url, init = {}) {
    const method = (init.method || "GET").toUpperCase();
    const path = new URL(url).pathname;
    const headers = init.headers || {};
    let body = init.body;
    if (typeof body === "string") {
      try {
        body = JSON.parse(body);
      } catch {
        /* leave as string */
      }
    }
    const req = { method, url, path, headers, body };
    calls.push(req);

    const handler = routes[`${method} ${path}`];
    const res = handler ? handler(req) : { status: 404, json: { error: "not found" } };
    const status = res.status ?? 200;
    const bytes = res.bytes !== undefined ? Uint8Array.from(res.bytes) : null;
    const text = res.text ?? (res.json !== undefined ? JSON.stringify(res.json) : "");
    return {
      ok: status >= 200 && status < 300,
      status,
      async text() {
        return text;
      },
      async arrayBuffer() {
        // Serve the declared bytes; fall back to encoding the text body so a
        // route that only set `text`/`json` still yields a usable buffer.
        const src = bytes ?? new TextEncoder().encode(text);
        return src.buffer.slice(src.byteOffset, src.byteOffset + src.byteLength);
      },
    };
  }
  return { fetch: fakeFetch, calls };
}

/** Standard env for a client that has both scopes enabled. */
export const TEST_ENV = {
  FIELDDRAFT_API_URL: "https://api.test",
  PO_SERVICE_TOKEN: "po-token-abc",
  CODING_SERVICE_TOKEN: "coding-token-xyz",
};
