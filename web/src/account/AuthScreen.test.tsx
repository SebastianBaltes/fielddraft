import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { AuthScreen } from "./AuthScreen";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

interface Handlers {
  onLogin?: (email: string, password: string) => Promise<void>;
  onRegister?: (input: {
    email: string;
    displayName: string;
    password: string;
    invite: string;
  }) => Promise<void>;
  onSwitch?: (mode: never) => void;
  onClose?: () => void;
}

function render(
  mode: "login" | "register" | "reset-request" | "reset-confirm",
  h: Handlers = {},
  prefill = { invite: "", reset: "" },
) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <AuthScreen
        mode={mode}
        invitePrefill={prefill.invite}
        resetPrefill={prefill.reset}
        onLogin={h.onLogin ?? (async () => {})}
        onRegister={h.onRegister ?? (async () => {})}
        onSwitch={h.onSwitch ?? (() => {})}
        onClose={h.onClose ?? (() => {})}
      />,
    );
  });
}

function setValue(testid: string, value: string) {
  const el = container.querySelector<HTMLInputElement>(
    `[data-testid="${testid}"]`,
  )!;
  Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!.call(el, value);
  act(() => {
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function submit() {
  const form = container.querySelector("form")!;
  act(() => {
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
  });
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("AuthScreen login", () => {
  it("calls onLogin with the entered credentials", async () => {
    const onLogin = vi.fn(async () => {});
    render("login", { onLogin });
    setValue("auth-email", "a@b.c");
    setValue("auth-password", "secret123");
    submit();
    await flush();
    expect(onLogin).toHaveBeenCalledWith("a@b.c", "secret123");
  });

  it("shows an inline error when onLogin rejects", async () => {
    const onLogin = vi.fn(async () => {
      throw new Error("invalid credentials");
    });
    render("login", { onLogin });
    setValue("auth-email", "a@b.c");
    setValue("auth-password", "nope");
    submit();
    await flush();
    const err = container.querySelector('[data-testid="auth-error"]');
    expect(err?.textContent).toContain("invalid credentials");
  });
});

describe("AuthScreen register", () => {
  it("prefills the invite token from the deep-link", () => {
    render("register", {}, { invite: "INV42", reset: "" });
    const invite = container.querySelector<HTMLInputElement>(
      '[data-testid="auth-invite"]',
    )!;
    expect(invite.value).toBe("INV42");
  });

  it("blocks submit with an inline error when no invite is present", async () => {
    const onRegister = vi.fn(async () => {});
    render("register", { onRegister });
    setValue("auth-email", "n@e.w");
    setValue("auth-password", "longpassword");
    submit();
    await flush();
    expect(onRegister).not.toHaveBeenCalled();
    expect(
      container.querySelector('[data-testid="auth-error"]')?.textContent,
    ).toContain("invite is required");
  });

  it("calls onRegister with the invite when present", async () => {
    const onRegister = vi.fn(async () => {});
    render("register", { onRegister });
    setValue("auth-email", "n@e.w");
    setValue("auth-display-name", "New");
    setValue("auth-password", "longpassword");
    setValue("auth-invite", "INV42");
    submit();
    await flush();
    expect(onRegister).toHaveBeenCalledWith({
      email: "n@e.w",
      displayName: "New",
      password: "longpassword",
      invite: "INV42",
    });
  });
});
