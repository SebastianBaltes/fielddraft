// Thin client over the WP6 agent / work-order API (server/agent.go, §8–§11).
//
// Both desktop agents authenticate with a least-privilege Service-Token sent as
// `Authorization: Bearer <token>` (§11). The scope is decided by WHICH token you
// pass, not by a request field — the server resolves po vs coding from the token
// itself. This client just attaches the right token per call.
//
// Design notes:
//   - Zero dependencies: plain ESM + global fetch (Node ≥18).
//   - `fetch` is injectable so the runners/tests can drive it without a network.
//   - Every non-2xx response throws an ApiError carrying status + parsed body,
//     so callers can branch on `err.status` (e.g. 404 → ticket gone, 401 →
//     token unset/disabled scope).
//   - Config comes from the environment; nothing is hard-coded.

/**
 * Error thrown for any non-2xx response from the agent API.
 * @property {number} status   HTTP status code.
 * @property {string} method   HTTP method of the failed request.
 * @property {string} path     Request path (relative to the API base).
 * @property {*}      body     Parsed JSON body if available, else the raw text.
 */
export class ApiError extends Error {
  constructor(status, method, path, body) {
    const detail =
      body && typeof body === "object" && body.error ? body.error : body;
    super(`agent API ${method} ${path} → ${status}${detail ? `: ${detail}` : ""}`);
    this.name = "ApiError";
    this.status = status;
    this.method = method;
    this.path = path;
    this.body = body;
  }
}

/**
 * Resolve the runtime config from env (or an explicit override, for tests).
 * @param {Record<string,string>} [env] defaults to process.env.
 */
export function configFromEnv(env = process.env) {
  return {
    apiUrl: (env.FIELDDRAFT_API_URL || "").replace(/\/+$/, ""),
    poToken: env.PO_SERVICE_TOKEN || "",
    codingToken: env.CODING_SERVICE_TOKEN || "",
  };
}

/**
 * Build an agent-API client.
 *
 * @param {object} [opts]
 * @param {string} [opts.apiUrl]        Base URL, e.g. https://fielddraft.example
 * @param {string} [opts.poToken]       PO_SERVICE_TOKEN (scope "po").
 * @param {string} [opts.codingToken]   CODING_SERVICE_TOKEN (scope "coding").
 * @param {typeof fetch} [opts.fetch]   Injected fetch (defaults to global fetch).
 * @param {Record<string,string>} [opts.env] env source for defaults.
 */
export function createApiClient(opts = {}) {
  const env = opts.env || process.env;
  const cfg = configFromEnv(env);
  const apiUrl = opts.apiUrl ?? cfg.apiUrl;
  const poToken = opts.poToken ?? cfg.poToken;
  const codingToken = opts.codingToken ?? cfg.codingToken;
  const fetchImpl = opts.fetch ?? globalThis.fetch;

  if (!apiUrl) {
    throw new Error(
      "FIELDDRAFT_API_URL is not set — the agent API base URL is required",
    );
  }
  if (typeof fetchImpl !== "function") {
    throw new Error(
      "no fetch available — pass { fetch } or run on Node ≥18 (global fetch)",
    );
  }

  // Pick the token for a scope; an empty token means that scope is disabled
  // server-side (constEq never matches "") — fail fast with a clear message.
  function tokenFor(scope) {
    const tok = scope === "po" ? poToken : codingToken;
    if (!tok) {
      throw new Error(
        `${scope === "po" ? "PO_SERVICE_TOKEN" : "CODING_SERVICE_TOKEN"} is not set — ` +
          `the ${scope} scope is disabled (the server rejects an empty token)`,
      );
    }
    return tok;
  }

  /**
   * Core request helper: attaches the scope's Bearer token, sends/parses JSON,
   * and throws ApiError on non-2xx.
   */
  async function request(scope, method, path, body) {
    const headers = {
      Authorization: `Bearer ${tokenFor(scope)}`,
      Accept: "application/json",
    };
    const init = { method, headers };
    if (body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(body);
    }

    const res = await fetchImpl(`${apiUrl}${path}`, init);
    const text = await res.text();
    let parsed = text;
    if (text) {
      try {
        parsed = JSON.parse(text);
      } catch {
        parsed = text; // non-JSON error body (e.g. http.Error plain text)
      }
    } else {
      parsed = null;
    }

    if (!res.ok) {
      throw new ApiError(res.status, method, path, parsed);
    }
    return parsed;
  }

  /**
   * Binary request helper: like `request` but returns the raw response bytes
   * (Uint8Array) instead of parsing JSON. Used for artifact-blob downloads,
   * which are content-addressed binary (screenshots, project files, …). Attaches
   * the scope's Bearer token just like `request`; on non-2xx it reads the body as
   * text so the ApiError still carries a readable server message.
   */
  async function requestBytes(scope, method, path) {
    const res = await fetchImpl(`${apiUrl}${path}`, {
      method,
      headers: { Authorization: `Bearer ${tokenFor(scope)}` },
    });
    if (!res.ok) {
      let body = null;
      try {
        const text = await res.text();
        try {
          body = JSON.parse(text);
        } catch {
          body = text;
        }
      } catch {
        /* body unreadable */
      }
      throw new ApiError(res.status, method, path, body);
    }
    const buf = await res.arrayBuffer();
    return new Uint8Array(buf);
  }

  return {
    /** Raw config (handy for logging which base URL the agent targets). */
    apiUrl,

    /**
     * GET /api/agent/queue — scope-aware actionable list (hydrated with thread +
     * artifacts; coding items also carry their work_order). Returns { scope, items }.
     * @param {"po"|"coding"} scope
     */
    getQueue(scope) {
      if (scope !== "po" && scope !== "coding") {
        throw new Error(`getQueue: scope must be "po" or "coding", got ${scope}`);
      }
      return request(scope, "GET", "/api/agent/queue");
    },

    /**
     * GET /api/tickets/{id}/artifacts/{artifactId} — download an artifact blob's
     * raw BYTES (the queue only carries artifact REFS, not the bytes). Returns a
     * Uint8Array. The `scope` picks the Bearer token — the PO uses "po" to pull a
     * screenshot, the coding agent uses "coding" for the full bundle.
     * @param {string} ticketId
     * @param {string} artifactId
     * @param {"po"|"coding"} [scope="coding"]
     * @returns {Promise<Uint8Array>}
     */
    fetchArtifact(ticketId, artifactId, scope = "coding") {
      return requestBytes(
        scope,
        "GET",
        `/api/tickets/${enc(ticketId)}/artifacts/${enc(artifactId)}`,
      );
    },

    /**
     * POST /api/tickets/{id}/verdict — the §8.3 PO verdict. On accepted the
     * server creates the work-order from `verdict.work_order`. (scope: po)
     * @param {string} ticketId
     * @param {object} verdict  { spam, malicious, vision_fit, confidence,
     *                            category, reason, public_reply, next_state,
     *                            work_order? }
     */
    postVerdict(ticketId, verdict) {
      return request("po", "POST", `/api/tickets/${enc(ticketId)}/verdict`, verdict);
    },

    /**
     * POST /api/tickets/{id}/agent-comment — add a thread comment.
     *   - authorType "po"  → public, leak-filtered (po scope only).
     *   - authorType "dev" → internal note (admin-only; the coding agent uses it).
     * The server maps authorType → the `kind` field it expects.
     * @param {string} ticketId
     * @param {{authorType:"po"|"dev", body:string}} comment
     */
    postAgentComment(ticketId, { authorType, body }) {
      // The server only lets the po scope post `po` (public) comments; `dev`
      // (internal) notes go through the coding scope.
      const scope = authorType === "po" ? "po" : "coding";
      return request(scope, "POST",
        `/api/tickets/${enc(ticketId)}/agent-comment`, { kind: authorType, body });
    },

    /**
     * PATCH /api/tickets/{id}/agent-state — drive a state transition directly
     * (validated against the §7.2 machine). (scope: po or coding)
     * @param {string} ticketId
     * @param {string} to       target state, e.g. "needs_info"
     * @param {"po"|"coding"} [scope="coding"]
     */
    patchState(ticketId, to, scope = "coding") {
      return request(scope, "PATCH", `/api/tickets/${enc(ticketId)}/agent-state`, {
        state: to,
      });
    },

    /**
     * POST /api/work-orders/{ticketId}/claim — claim the work-order; the server
     * sets picked_up_at and drives the ticket accepted→in_progress. (scope: coding)
     * @param {string} ticketId
     */
    claim(ticketId) {
      return request("coding", "POST", `/api/work-orders/${enc(ticketId)}/claim`);
    },

    /**
     * POST /api/work-orders/{ticketId}/complete — complete the work-order and
     * transition the ticket (default → resolved). (scope: coding)
     * @param {string} ticketId
     * @param {{deployId?:string, nextState?:string}} [opts]
     */
    complete(ticketId, opts = {}) {
      const body = {};
      if (opts.deployId !== undefined) body.deploy_id = opts.deployId;
      if (opts.nextState !== undefined) body.next_state = opts.nextState;
      return request("coding", "POST", `/api/work-orders/${enc(ticketId)}/complete`, body);
    },

    /**
     * POST /api/deploys — record a rollout (audit trail for rollback, §10.3).
     * (scope: coding)
     * @param {{version:string, gitSha?:string, ticketIds?:string[], status?:string}} d
     */
    recordDeploy(d) {
      return request("coding", "POST", "/api/deploys", {
        version: d.version,
        git_sha: d.gitSha,
        ticket_ids: d.ticketIds,
        status: d.status,
      });
    },

    /**
     * GET /api/deploys — the deploy audit list (for rollback decisions).
     * (scope: coding — also served to admin sessions, but the agent uses coding)
     */
    listDeploys() {
      return request("coding", "GET", "/api/deploys");
    },
  };
}

// Encode a path segment so an id with odd characters can't break out of the path.
function enc(segment) {
  return encodeURIComponent(String(segment));
}

export default createApiClient;
