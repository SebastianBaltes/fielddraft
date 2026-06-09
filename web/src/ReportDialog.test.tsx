import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ReportDialog } from "./ReportDialog";
import type { KernelStore } from "./store";

// captureWindow relies on html-to-image rasterizing real DOM, which jsdom
// can't do — stub it so the dialog mounts cleanly. submitTicket stays real
// (it just calls the stubbed fetch).
vi.mock("./reportTicket", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./reportTicket")>()),
  captureWindow: vi.fn(async () => null),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

// Minimal store stub exposing only what ReportDialog touches.
const stubStore = {
  exportProjectJson: () => '{"schemaVersion":1,"tabs":[]}',
} as unknown as KernelStore;

let container: HTMLDivElement;
let root: Root;

function render(onClose = vi.fn()) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<ReportDialog store={stubStore} onClose={onClose} />);
  });
  return onClose;
}

// React tracks input state via the native value setter; mutate through it so
// the synthetic onChange fires (the same trick testing-library uses).
function typeInto(selector: string, value: string) {
  const el = container.querySelector<HTMLInputElement | HTMLTextAreaElement>(selector)!;
  const proto = el instanceof HTMLTextAreaElement
    ? HTMLTextAreaElement.prototype
    : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
  act(() => {
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  // __APP_VERSION__ is a build-time define; provide it for the test runtime.
  vi.stubGlobal("__APP_VERSION__", "0.0.0-test");
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({
      ok: true,
      json: async () => ({ ticket_id: "20260609-000000-test" }),
    })),
  );
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

describe("ReportDialog", () => {
  it("disables submit until a title is entered", () => {
    render();
    const submit = container.querySelector<HTMLButtonElement>(
      '[data-testid="report-submit"]',
    )!;
    expect(submit.disabled).toBe(true);
    typeInto('[data-testid="report-title"]', "Something broke");
    expect(submit.disabled).toBe(false);
  });

  it("POSTs a ticket and closes without a confirmation step", async () => {
    const onClose = render();
    typeInto('[data-testid="report-title"]', "Edges missing");
    typeInto('[data-testid="report-description"]', "Repro steps here");

    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('[data-testid="report-submit"]')!
        .click();
    });

    const fetchMock = fetch as unknown as ReturnType<typeof vi.fn>;
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, opts] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/tickets");
    const body = JSON.parse((opts as RequestInit).body as string);
    expect(body.title).toBe("Edges missing");
    expect(body.description).toBe("Repro steps here");
    expect(body.projectJson).toContain("schemaVersion");
    expect(typeof body.version).toBe("string");
    expect(typeof body.timestamp).toBe("string");

    // No "Ticket saved" panel — the dialog closes straight away.
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("surfaces a server error without closing", async () => {
    const onClose = render();
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 500 })));
    typeInto('[data-testid="report-title"]', "Boom");

    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('[data-testid="report-submit"]')!
        .click();
    });

    expect(container.textContent).toContain("Could not save ticket");
    expect(onClose).not.toHaveBeenCalled();
  });
});
