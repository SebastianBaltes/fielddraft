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

// Minimal store stub exposing only what ReportDialog touches. on3k: the
// dialog now ships the project as a .fdraft ZIP (raw bytes → base64), so the
// stub returns a couple of sentinel bytes we can assert survived the encode.
const stubStore = {
  exportProjectFdraft: () => new Uint8Array([0x50, 0x4b, 0x03, 0x04]),
  uiStateSnapshot: () => ({ camera: null, selection: [] }),
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

// FileReader (jsdom) delivers results on the macrotask queue — flush it so
// the async addFiles pipeline lands before asserting.
async function flushTasks() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
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
  // ticket xqll: the description is the only required field — there is no
  // title input anymore.
  it("disables submit until a description is entered", () => {
    render();
    const submit = container.querySelector<HTMLButtonElement>(
      '[data-testid="report-submit"]',
    )!;
    expect(submit.disabled).toBe(true);
    typeInto('[data-testid="report-description"]', "Something broke");
    expect(submit.disabled).toBe(false);
  });

  it("POSTs a ticket and closes without a confirmation step", async () => {
    const onClose = render();
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
    // ticket xqll: no title is sent anymore.
    expect(body.title).toBeUndefined();
    expect(body.description).toBe("Repro steps here");
    // on3k: project rides along as a base64 .fdraft ZIP (PK\x03\x04 → "UEsDBA==").
    expect(body.projectJson).toBeUndefined();
    expect(body.projectFdraft).toBe("UEsDBA==");
    expect(typeof body.version).toBe("string");
    expect(typeof body.timestamp).toBe("string");

    // No "Ticket saved" panel — the dialog closes straight away.
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  // ticket ttzl: the panel minimizes to its header and back, keeping the
  // already-typed description (body stays mounted, just hidden).
  it("minimizes and restores without losing the description", async () => {
    render();
    typeInto('[data-testid="report-description"]', "Half-written report");

    const body = () =>
      container.querySelector<HTMLElement>('[data-testid="report-dialog"] .body')!;
    expect(body().hidden).toBe(false);

    const minBtn = container.querySelector<HTMLButtonElement>(
      '[data-testid="report-minimize"]',
    )!;
    await act(async () => minBtn.click());
    expect(body().hidden).toBe(true);

    await act(async () => minBtn.click());
    expect(body().hidden).toBe(false);
    expect(
      container.querySelector<HTMLTextAreaElement>(
        '[data-testid="report-description"]',
      )!.value,
    ).toBe("Half-written report");
  });

  it("attaches picked files, shows thumbs and removes via the X", async () => {
    render();
    const file = new File(["hello"], "notes.txt", { type: "text/plain" });
    const input = container.querySelector<HTMLInputElement>(
      '[data-testid="report-file-input"]',
    )!;
    Object.defineProperty(input, "files", { value: [file] });
    await act(async () => {
      input.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await flushTasks();

    const thumbs = container.querySelector('[data-testid="report-attachments"]')!;
    expect(thumbs.textContent).toContain("notes.txt");
    // Non-image files render as an extension chip, not an <img>.
    expect(thumbs.querySelector("img")).toBeNull();
    expect(thumbs.textContent).toContain("TXT");

    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('[data-testid="report-attachment-remove-0"]')!
        .click();
    });
    expect(container.querySelector('[data-testid="report-attachments"]')).toBeNull();
  });

  it("attaches pasted images via Ctrl+V and submits them in the payload", async () => {
    render();
    typeInto('[data-testid="report-description"]', "With paste");

    const img = new File([new Uint8Array([137, 80, 78, 71])], "", {
      type: "image/png",
    });
    const pasteEvent = new Event("paste", { bubbles: true }) as ClipboardEvent;
    Object.defineProperty(pasteEvent, "clipboardData", {
      value: { files: [img] },
    });
    await act(async () => {
      window.dispatchEvent(pasteEvent);
    });
    await flushTasks();

    const thumbs = container.querySelector('[data-testid="report-attachments"]')!;
    // Clipboard images get a synthetic name and an <img> preview.
    expect(thumbs.textContent).toContain("pasted-1.png");
    expect(thumbs.querySelector("img")).not.toBeNull();

    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('[data-testid="report-submit"]')!
        .click();
    });
    const fetchMock = fetch as unknown as ReturnType<typeof vi.fn>;
    const body = JSON.parse(
      (fetchMock.mock.calls[0][1] as RequestInit).body as string,
    );
    expect(body.attachments).toHaveLength(1);
    expect(body.attachments[0].name).toBe("pasted-1.png");
    expect(body.attachments[0].mime).toBe("image/png");
    expect(body.attachments[0].dataUrl).toMatch(/^data:/);
  });

  it("rejects oversized attachments with an inline error", async () => {
    render();
    const big = new File([""], "huge.bin", { type: "application/octet-stream" });
    Object.defineProperty(big, "size", { value: 9 * 1024 * 1024 });
    const input = container.querySelector<HTMLInputElement>(
      '[data-testid="report-file-input"]',
    )!;
    Object.defineProperty(input, "files", { value: [big] });
    await act(async () => {
      input.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await flushTasks();

    expect(container.querySelector('[data-testid="report-attachments"]')).toBeNull();
    expect(container.textContent).toContain("huge.bin");
    expect(container.textContent).toContain("larger than 8 MB");
  });

  it("surfaces a server error without closing", async () => {
    const onClose = render();
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 500 })));
    typeInto('[data-testid="report-description"]', "Boom");

    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('[data-testid="report-submit"]')!
        .click();
    });

    expect(container.textContent).toContain("Could not save ticket");
    expect(onClose).not.toHaveBeenCalled();
  });
});
