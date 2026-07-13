import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { TicketsView } from "./TicketsView";
import * as platform from "../api/platform";
import type {
  TicketComment,
  TicketDetail,
  TicketSummary,
  User,
} from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const USER: User = {
  id: "u1",
  email: "a@b.c",
  display_name: "Ada",
  role: "user",
  created_at: "2026-01-01T00:00:00Z",
};

function summary(over: Partial<TicketSummary> = {}): TicketSummary {
  return {
    id: "t1",
    number: 1,
    author_id: "u1",
    title: "Crash on extrude",
    state: "triage",
    category: "bug",
    created_at: "2026-05-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
    ...over,
  };
}

function detail(over: Partial<TicketDetail> = {}): TicketDetail {
  return {
    id: "t1",
    number: 1,
    author_id: "u1",
    title: "Crash on extrude",
    body: "It explodes",
    state: "triage",
    category: "bug",
    po_spam: false,
    po_malicious: false,
    po_vision_fit: null,
    po_reason: null,
    po_verdict_at: null,
    created_at: "2026-05-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
    comments: [],
    artifacts: [],
    ...over,
  };
}

function comment(over: Partial<TicketComment> = {}): TicketComment {
  return {
    id: "c1",
    ticket_id: "t1",
    author_type: "user",
    author_id: "u1",
    internal: false,
    body: "a comment",
    created_at: "2026-06-02T00:00:00Z",
    withheld: false,
    ...over,
  };
}

let container: HTMLDivElement;
let root: Root;

function mount(props: Partial<Parameters<typeof TicketsView>[0]> = {}) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <TicketsView
        user={USER}
        isAdmin={false}
        onLoadFdraft={() => {}}
        ticketId={null}
        onNavigate={() => {}}
        {...props}
      />,
    );
  });
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function byTestId<T extends HTMLElement = HTMLElement>(id: string): T | null {
  return container.querySelector<T>(`[data-testid="${id}"]`);
}

beforeEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("TicketsView — list", () => {
  it("lists tickets sorted by activity (newest first)", async () => {
    vi.spyOn(platform.platformApi, "listTickets").mockResolvedValue([
      summary({ id: "old", updated_at: "2026-01-01T00:00:00Z" }),
      summary({ id: "new", updated_at: "2026-06-01T00:00:00Z" }),
    ]);
    mount();
    await flush();
    const rows = container.querySelectorAll('[data-testid^="ticket-row-"]');
    expect(rows).toHaveLength(2);
    expect(rows[0].getAttribute("data-testid")).toBe("ticket-row-new");
  });

  it("toggling 'only mine' refilters with author=me", async () => {
    const list = vi
      .spyOn(platform.platformApi, "listTickets")
      .mockResolvedValue([summary()]);
    mount();
    await flush();
    expect(list).toHaveBeenLastCalledWith(
      expect.objectContaining({ author: undefined }),
    );
    const toggle = byTestId<HTMLInputElement>("tickets-only-mine")!;
    act(() => toggle.click());
    await flush();
    expect(list).toHaveBeenLastCalledWith(
      expect.objectContaining({ author: "me" }),
    );
  });

  it("shows an empty state when nothing matches", async () => {
    vi.spyOn(platform.platformApi, "listTickets").mockResolvedValue([]);
    mount();
    await flush();
    expect(byTestId("tickets-empty")).toBeTruthy();
  });

  it("surfaces a list error", async () => {
    vi.spyOn(platform.platformApi, "listTickets").mockRejectedValue(
      new platform.ApiError(500, "boom"),
    );
    mount();
    await flush();
    expect(byTestId("tickets-error")?.textContent).toContain("boom");
  });

  it("paginates with an adjustable, persisted page size", async () => {
    const many = Array.from({ length: 30 }, (_, i) =>
      summary({
        id: `t${i}`,
        number: i + 1,
        // descending updated_at keeps t0…t29 in order under the default sort
        updated_at: `2026-06-${String(30 - i).padStart(2, "0")}T00:00:00Z`,
      }),
    );
    vi.spyOn(platform.platformApi, "listTickets").mockResolvedValue(many);
    mount();
    await flush();

    const rows = () =>
      container.querySelectorAll('[data-testid^="ticket-row-"]');

    // Default page size 25 → first page shows 25 of 30.
    expect(rows()).toHaveLength(25);
    expect(byTestId("ticket-total-count")?.textContent).toContain("30 issues");
    expect(byTestId("ticket-page-status")?.textContent).toBe("Page 1 of 2");
    expect(byTestId<HTMLButtonElement>("ticket-page-prev")!.disabled).toBe(true);

    // Next → the remaining 5.
    act(() => byTestId<HTMLButtonElement>("ticket-page-next")!.click());
    await flush();
    expect(rows()).toHaveLength(5);
    expect(byTestId("ticket-page-status")?.textContent).toBe("Page 2 of 2");
    expect(byTestId<HTMLButtonElement>("ticket-page-next")!.disabled).toBe(true);

    // Changing the page size snaps back to page 1, re-slices, and persists.
    const select = byTestId<HTMLSelectElement>("tickets-page-size")!;
    const setValue = Object.getOwnPropertyDescriptor(
      HTMLSelectElement.prototype,
      "value",
    )!.set!;
    act(() => {
      setValue.call(select, "10");
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await flush();
    expect(rows()).toHaveLength(10);
    expect(byTestId("ticket-page-status")?.textContent).toBe("Page 1 of 3");
    expect(localStorage.getItem("fielddraft.tickets.pageSize")).toBe("10");
  });
});

describe("TicketsView — detail", () => {
  it("opens a ticket and renders its body + thread", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({ comments: [comment({ body: "first reply" })] }),
    );
    mount({ ticketId: "t1" });
    await flush();
    expect(byTestId("detail-body")?.textContent).toBe("It explodes");
    expect(byTestId("ticket-thread")?.textContent).toContain("first reply");
  });

  it("hides internal dev comments from non-admins (UI guard)", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({
        comments: [
          comment({ id: "pub", author_type: "po", body: "public note" }),
          comment({
            id: "secret",
            author_type: "dev",
            internal: true,
            body: "SECRET dev note",
          }),
        ],
      }),
    );
    mount({ ticketId: "t1", isAdmin: false });
    await flush();
    expect(byTestId("comment-pub")).toBeTruthy();
    expect(byTestId("comment-secret")).toBeNull();
  });

  it("shows internal dev comments to admins", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({
        comments: [
          comment({
            id: "secret",
            author_type: "dev",
            internal: true,
            body: "SECRET dev note",
          }),
        ],
      }),
    );
    mount({ ticketId: "t1", isAdmin: true });
    await flush();
    expect(byTestId("comment-secret")).toBeTruthy();
  });

  it("posts a comment and reloads the thread", async () => {
    const get = vi
      .spyOn(platform.platformApi, "getTicket")
      .mockResolvedValue(detail());
    const add = vi
      .spyOn(platform.platformApi, "addComment")
      .mockResolvedValue(comment({ id: "new", body: "my reply" }));
    mount({ ticketId: "t1" });
    await flush();
    const input = byTestId<HTMLTextAreaElement>("comment-input")!;
    const setValue = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )!.set!;
    act(() => {
      setValue.call(input, "my reply");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    act(() => byTestId<HTMLButtonElement>("comment-submit")!.click());
    await flush();
    expect(add).toHaveBeenCalledWith("t1", "my reply");
    // reload → a second getTicket.
    expect(get).toHaveBeenCalledTimes(2);
  });

  it("loads the .fdraft artifact via onLoadFdraft", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({
        artifacts: [
          {
            id: "a1",
            ticket_id: "t1",
            kind: "fdraft",
            blob_ref: "ref",
            mime: "application/zip",
            size: 4,
            name: "project.fdraft",
            created_at: "2026-06-01T00:00:00Z",
          },
        ],
      }),
    );
    const bytes = new Uint8Array([80, 75, 3, 4]).buffer;
    vi.spyOn(platform.platformApi, "fetchArtifact").mockResolvedValue(bytes);
    const onLoadFdraft = vi.fn();
    mount({ ticketId: "t1", onLoadFdraft });
    await flush();
    act(() => byTestId<HTMLButtonElement>("artifact-load-project")!.click());
    await flush();
    expect(platform.platformApi.fetchArtifact).toHaveBeenCalledWith("t1", "a1");
    expect(onLoadFdraft).toHaveBeenCalledWith(bytes);
  });
});

describe("TicketsView — list-only (WP13)", () => {
  it("has no New-Issue button or create form (issues are filed via Report Issue)", async () => {
    vi.spyOn(platform.platformApi, "listTickets").mockResolvedValue([]);
    mount();
    await flush();
    expect(byTestId("tickets-new")).toBeNull();
    expect(byTestId("create-title")).toBeNull();
    expect(byTestId("create-submit")).toBeNull();
  });
});

/* State picker + delete (ticket #345). The options themselves are the subject of
 * settableStates in ticketView.test.ts; here it's the wiring. */
describe("TicketsView — set state / delete", () => {
  const options = () =>
    Array.from(
      byTestId<HTMLSelectElement>("detail-set-state")!.options,
      (o) => o.value,
    );

  function pick(state: string) {
    const select = byTestId<HTMLSelectElement>("detail-set-state")!;
    select.value = state;
    act(() => select.dispatchEvent(new Event("change", { bubbles: true })));
  }

  it("lets an admin set any state directly — even one the graph forbids", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({ state: "triage", author_id: "someone-else" }),
    );
    const update = vi
      .spyOn(platform.platformApi, "updateTicket")
      .mockResolvedValue(detail({ state: "resolved" }));
    mount({ ticketId: "t1", isAdmin: true });
    await flush();

    // triage → resolved is not a machine edge, but the admin may jump there.
    expect(options()).toContain("resolved");
    expect(options()).not.toContain("triage"); // the current state is a no-op
    pick("resolved");
    await flush();

    expect(update).toHaveBeenCalledWith("t1", { state: "resolved" });
    expect(byTestId("detail-state")!.textContent).toBe("Resolved");
  });

  it("offers the author only their safe subset", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({ state: "triage" }), // author_id = u1 = USER
    );
    mount({ ticketId: "t1" });
    await flush();
    expect(options().sort()).toEqual(["", "backlog", "closed"]);
  });

  it("offers a stranger no state picker and no delete", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({ author_id: "someone-else" }),
    );
    mount({ ticketId: "t1" });
    await flush();
    expect(byTestId("detail-set-state")).toBeNull();
    expect(byTestId("detail-delete-ticket")).toBeNull();
  });

  it("deletes after a confirm and navigates back", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(detail());
    const del = vi
      .spyOn(platform.platformApi, "deleteTicket")
      .mockResolvedValue(undefined);
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const onNavigate = vi.fn();
    mount({ ticketId: "t1", onNavigate });
    await flush();

    act(() => byTestId<HTMLButtonElement>("detail-delete-ticket")!.click());
    await flush();

    expect(del).toHaveBeenCalledWith("t1");
    expect(onNavigate).toHaveBeenCalledWith(null); // back to the list
  });

  it("does not delete when the confirm is declined", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(detail());
    const del = vi
      .spyOn(platform.platformApi, "deleteTicket")
      .mockResolvedValue(undefined);
    vi.spyOn(window, "confirm").mockReturnValue(false);
    mount({ ticketId: "t1" });
    await flush();

    act(() => byTestId<HTMLButtonElement>("detail-delete-ticket")!.click());
    await flush();

    expect(del).not.toHaveBeenCalled();
  });

  it("surfaces a failed state change and keeps the old state", async () => {
    vi.spyOn(platform.platformApi, "getTicket").mockResolvedValue(
      detail({ state: "triage" }),
    );
    vi.spyOn(platform.platformApi, "updateTicket").mockRejectedValue(
      new Error("illegal state transition"),
    );
    mount({ ticketId: "t1" });
    await flush();
    pick("closed");
    await flush();
    expect(byTestId("detail-error")!.textContent).toContain(
      "illegal state transition",
    );
    expect(byTestId("detail-state")!.textContent).toBe("Triage");
  });
});
