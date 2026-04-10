import { describe, expect, it } from "vitest";
import type { FdInnerApi } from "./workerSandbox";
import {
  dispatchFdCall,
  handleFdCallRequest,
  isFdCallRequest,
  type FdCallRequest,
} from "./workerRpc";

function fakeApi(): FdInnerApi {
  const logs: unknown[][] = [];
  return {
    version: "1.2.3",
    log: (...args) => {
      logs.push(args);
    },
    get logs() {
      return logs;
    },
    ping: (msg) => `pong: ${msg}`,
    echo: (value) => value,
  };
}

describe("workerRpc.isFdCallRequest", () => {
  it("accepts a well-formed request", () => {
    const req: FdCallRequest = {
      type: "fd_call",
      requestId: 1,
      method: "ping",
      args: ["hi"],
    };
    expect(isFdCallRequest(req)).toBe(true);
  });

  it("rejects unrelated messages", () => {
    expect(isFdCallRequest({ type: "rebuild", requestId: 1 })).toBe(false);
    expect(isFdCallRequest(null)).toBe(false);
    expect(isFdCallRequest("fd_call")).toBe(false);
    expect(isFdCallRequest({ type: "fd_call" })).toBe(false);
  });

  it("rejects malformed fd_call shapes", () => {
    expect(
      isFdCallRequest({ type: "fd_call", requestId: "1", method: "x", args: [] }),
    ).toBe(false);
    expect(
      isFdCallRequest({ type: "fd_call", requestId: 1, method: 42, args: [] }),
    ).toBe(false);
    expect(
      isFdCallRequest({ type: "fd_call", requestId: 1, method: "x", args: {} }),
    ).toBe(false);
  });
});

describe("workerRpc.dispatchFdCall", () => {
  it("calls a function method with positional args", () => {
    const api = fakeApi();
    const result = dispatchFdCall(api, "ping", ["hello"]);
    expect(result).toBe("pong: hello");
  });

  it("echoes a structured-cloneable argument unchanged", () => {
    const api = fakeApi();
    const payload = { a: 1, b: [2, 3], c: { nested: true } };
    const result = dispatchFdCall(api, "echo", [payload]);
    expect(result).toEqual(payload);
  });

  it("returns a non-function property as if it were a zero-arg getter", () => {
    const api = fakeApi();
    expect(dispatchFdCall(api, "version", [])).toBe("1.2.3");
  });

  it("throws on an unknown method", () => {
    const api = fakeApi();
    expect(() => dispatchFdCall(api, "nonexistent", [])).toThrow(
      /unknown fd method: nonexistent/,
    );
  });

  it("propagates errors thrown by the inner method", () => {
    const api: FdInnerApi = {
      ...fakeApi(),
      ping: () => {
        throw new Error("boom");
      },
    };
    expect(() => dispatchFdCall(api, "ping", ["x"])).toThrow("boom");
  });

  it("invokes the method with the api as receiver so `this` is stable", () => {
    let captured: unknown = null;
    const api: FdInnerApi = {
      ...fakeApi(),
      ping: function (msg: string) {
        captured = this;
        return msg;
      },
    };
    dispatchFdCall(api, "ping", ["x"]);
    expect(captured).toBe(api);
  });
});

describe("workerRpc.handleFdCallRequest", () => {
  it("wraps a successful dispatch in an ok response", () => {
    const api = fakeApi();
    const resp = handleFdCallRequest(api, {
      type: "fd_call",
      requestId: 7,
      method: "ping",
      args: ["world"],
    });
    expect(resp).toEqual({
      type: "fd_result",
      requestId: 7,
      ok: true,
      value: "pong: world",
    });
  });

  it("wraps a thrown error in an ok:false response with the message", () => {
    const api: FdInnerApi = {
      ...fakeApi(),
      ping: () => {
        throw new Error("nope");
      },
    };
    const resp = handleFdCallRequest(api, {
      type: "fd_call",
      requestId: 9,
      method: "ping",
      args: ["x"],
    });
    expect(resp).toEqual({
      type: "fd_result",
      requestId: 9,
      ok: false,
      error: "nope",
    });
  });

  it("preserves the request id in the response for correlation", () => {
    const api = fakeApi();
    const resp = handleFdCallRequest(api, {
      type: "fd_call",
      requestId: 12345,
      method: "version",
      args: [],
    });
    expect(resp.requestId).toBe(12345);
  });

  it("stringifies non-Error throwables", () => {
    const api: FdInnerApi = {
      ...fakeApi(),
      // eslint-disable-next-line @typescript-eslint/no-throw-literal
      ping: () => {
        throw "bare string thrown";
      },
    };
    const resp = handleFdCallRequest(api, {
      type: "fd_call",
      requestId: 1,
      method: "ping",
      args: ["x"],
    });
    expect(resp.ok).toBe(false);
    if (!resp.ok) expect(resp.error).toBe("bare string thrown");
  });
});
