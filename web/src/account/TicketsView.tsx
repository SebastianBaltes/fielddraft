/**
 * TicketsView — the GitHub-Issues-like ticket UI (WP4, §7.4; WP13 Issues tab).
 *
 * A self-contained master/detail screen (the Overview's Issues tab):
 *   - the LIST: filter bar (state / category / free-text) + an "only mine"
 *     toggle, state badges, category, author, last activity — sorted by
 *     activity (newest first). It is LIST-ONLY: issues are filed via the
 *     👤 menu's "Report Issue…", so there is no New-Issue button or create
 *     form here — the PO agent assigns the title / category / status;
 *   - the DETAIL: the PO-assigned title + category + state, the body, artifact
 *     previews (screenshot thumbnail + "Load project" for the `.fdraft`, other
 *     attachments listed), the comment thread (User / PO / System personas;
 *     internal `dev` notes only for admins — guarded again in the UI), and a
 *     comment box (edit/close your own ticket too).
 *
 * It never reaches into the kernel store directly: App.tsx passes `onLoadFdraft`
 * (load a ticket's `.fdraft` artifact into the live workspace), keeping guest
 * mode decoupled. It is a persistent Overview tab — there is NO close (×)
 * button.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  platformApi,
  artifactUrl,
  type TicketArtifact,
  type TicketComment,
  type TicketDetail,
  type TicketListFilter,
  type TicketState,
  type TicketSummary,
  type User,
} from "../api/platform";
import { accountErrorMessage } from "./useAccount";
import { linkify } from "./linkify";
import {
  commentVisible,
  formatDateTime,
  personaLabel,
  personaTag,
  stateBucket,
  stateLabel,
  ticketRef,
} from "./ticketView";

interface TicketsViewProps {
  /** The logged-in user (null while guest — listing needs a session). */
  user: User | null;
  isAdmin: boolean;
  /** Load a ticket's `.fdraft` artifact bytes into the live workspace as an
   *  unbound copy (App.tsx wires this to the kernel store). */
  onLoadFdraft(bytes: ArrayBuffer): void;
  /** Controlled master/detail selection: the open ticket id, or null for
   *  the list. Owned by the account state so the URL hash can drive it
   *  (#/issues/<id>, ticket 3032c29b). */
  ticketId: string | null;
  /** Navigate list ↔ detail (routes through account.openTicket → URL). */
  onNavigate(id: string | null): void;
}

const STATES: TicketState[] = [
  "triage",
  "needs_info",
  "accepted",
  "in_progress",
  "deploying",
  "resolved",
  "closed",
  "declined",
  "backlog",
  "rejected_spam",
  "rejected_malicious",
];

export function TicketsView({
  user,
  isAdmin,
  onLoadFdraft,
  ticketId,
  onNavigate,
}: TicketsViewProps) {
  // Controlled master/detail: the list or a single ticket id. There is no
  // create mode — issues are filed via the "Report Issue…" flow. The key
  // remounts the detail panel when the id changes (hash navigation between
  // two tickets must not keep stale panel state).
  if (ticketId) {
    return (
      <TicketDetailPanel
        key={ticketId}
        id={ticketId}
        user={user}
        isAdmin={isAdmin}
        onLoadFdraft={onLoadFdraft}
        onBack={() => onNavigate(null)}
      />
    );
  }

  return <TicketList user={user} onOpen={(id) => onNavigate(id)} />;
}

/* ── list ───────────────────────────────────────────────────────── */

interface TicketListProps {
  user: User | null;
  onOpen(id: string): void;
}

/** The click-sortable columns of the issue list (ticket a7b63b6; the `#`
 *  reference column is ticket 066230d9). */
type SortKey = "number" | "state" | "title" | "updated";
const SORT_COLUMNS: { key: SortKey; label: string }[] = [
  { key: "number", label: "#" },
  { key: "state", label: "State" },
  { key: "title", label: "Title" },
  { key: "updated", label: "Updated" },
];

/** Page-size options for the issue list; `0` = "All" on one page. The chosen
 *  size persists in localStorage across sessions. */
const PAGE_SIZES = [10, 25, 50, 100];
const DEFAULT_PAGE_SIZE = 25;
const PAGE_SIZE_KEY = "fielddraft.tickets.pageSize";

function loadPageSize(): number {
  try {
    const raw = localStorage.getItem(PAGE_SIZE_KEY);
    if (raw !== null) {
      const n = Number(raw);
      if (n === 0 || PAGE_SIZES.includes(n)) return n;
    }
  } catch {
    /* localStorage unavailable — use the default */
  }
  return DEFAULT_PAGE_SIZE;
}

function TicketList({ user, onOpen }: TicketListProps) {
  const [tickets, setTickets] = useState<TicketSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [onlyMine, setOnlyMine] = useState(false);
  const [stateFilter, setStateFilter] = useState<TicketState | "">("");
  const [query, setQuery] = useState("");
  // Sort column + direction (ticket a7b63b6). Default: most-recent activity.
  const [sortKey, setSortKey] = useState<SortKey>("updated");
  const [sortAsc, setSortAsc] = useState(false);
  // Client-side pagination with an adjustable, persisted page size (0 = all).
  const [pageSize, setPageSize] = useState<number>(loadPageSize);
  const [page, setPage] = useState(1);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    const filter: TicketListFilter = {
      author: onlyMine ? "me" : undefined,
      state: stateFilter || undefined,
      q: query || undefined,
    };
    try {
      const list = await platformApi.listTickets(filter);
      setTickets(list);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load tickets"));
    } finally {
      setLoading(false);
    }
  }, [onlyMine, stateFilter, query]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // A changed filter can shrink the result set — snap back to the first page.
  useEffect(() => {
    setPage(1);
  }, [onlyMine, stateFilter, query]);

  // Client-side sort by the chosen column (the server returns the filtered
  // set; the header toggles key + direction — ticket a7b63b6).
  const sortedTickets = useMemo(() => {
    const key = (t: TicketSummary): string => {
      switch (sortKey) {
        case "number":
          // localeCompare with numeric:true orders "#2" before "#10".
          return String(t.number);
        case "state":
          return stateLabel(t.state);
        case "title":
          return t.title;
        case "updated":
          return t.updated_at;
      }
    };
    const dir = sortAsc ? 1 : -1;
    return [...tickets].sort(
      (a, b) => dir * key(a).localeCompare(key(b), undefined, { numeric: true }),
    );
  }, [tickets, sortKey, sortAsc]);

  // Slice the sorted set into the current page. pageSize 0 = show all on one.
  const total = sortedTickets.length;
  const perPage = pageSize > 0 ? pageSize : Math.max(total, 1);
  const totalPages = Math.max(1, Math.ceil(total / perPage));
  const safePage = Math.min(Math.max(1, page), totalPages);
  const pagedTickets = sortedTickets.slice(
    (safePage - 1) * perPage,
    safePage * perPage,
  );

  const changePageSize = (v: number) => {
    setPageSize(v);
    setPage(1);
    try {
      localStorage.setItem(PAGE_SIZE_KEY, String(v));
    } catch {
      /* ignore persistence failure */
    }
  };

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortAsc((v) => !v);
    } else {
      setSortKey(key);
      // Dates default newest-first; text columns default A→Z.
      setSortAsc(key !== "updated");
    }
  };

  return (
    <div className="fd-tickets" data-testid="tickets">
      <div className="filter-bar" data-testid="tickets-filters">
        <label className="only-mine">
          <input
            type="checkbox"
            data-testid="tickets-only-mine"
            checked={onlyMine}
            disabled={!user}
            onChange={(e) => setOnlyMine(e.target.checked)}
          />
          <span>Only mine</span>
        </label>
        <select
          className="filter-select"
          data-testid="tickets-state-filter"
          aria-label="Filter by state"
          value={stateFilter}
          onChange={(e) => setStateFilter(e.target.value as TicketState | "")}
        >
          <option value="">All states</option>
          {STATES.map((s) => (
            <option key={s} value={s}>
              {stateLabel(s)}
            </option>
          ))}
        </select>
        <input
          type="search"
          className="filter-search"
          data-testid="tickets-search"
          placeholder="Search title / body…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      {error && (
        <p className="error" role="alert" data-testid="tickets-error">
          {error}
        </p>
      )}

      {loading ? (
        <p className="empty" data-testid="tickets-loading">
          Loading…
        </p>
      ) : tickets.length === 0 ? (
        <p className="empty" data-testid="tickets-empty">
          No issues match these filters.
        </p>
      ) : (
        <>
        <div className="ticket-table" data-testid="ticket-list">
          <div className="ticket-head" role="row">
            {SORT_COLUMNS.map((col) => (
              <button
                key={col.key}
                type="button"
                className={`col-head col-${col.key}${
                  sortKey === col.key ? " active" : ""
                }`}
                data-testid={`ticket-sort-${col.key}`}
                aria-sort={
                  sortKey === col.key
                    ? sortAsc
                      ? "ascending"
                      : "descending"
                    : "none"
                }
                onClick={() => onSort(col.key)}
              >
                {col.label}
                <span className="sort-arrow" aria-hidden="true">
                  {sortKey === col.key ? (sortAsc ? "▲" : "▼") : ""}
                </span>
              </button>
            ))}
          </div>
          <ul className="ticket-rows">
            {pagedTickets.map((t) => (
            <li
              key={t.id}
              className="ticket-row"
              data-testid={`ticket-row-${t.id}`}
            >
              <button
                type="button"
                className="row-open"
                data-testid={`ticket-open-${t.id}`}
                onClick={() => onOpen(t.id)}
              >
                <span className="row-number" data-testid={`ticket-number-${t.id}`}>
                  {ticketRef(t.number)}
                </span>
                <span
                  className={`badge state-${stateBucket(t.state)}`}
                  data-testid={`ticket-state-${t.id}`}
                >
                  {stateLabel(t.state)}
                </span>
                <span className="row-title" title={t.title}>
                  {t.title}
                </span>
                <span className="row-meta">
                  {formatDateTime(t.updated_at)}
                </span>
              </button>
            </li>
            ))}
          </ul>
        </div>
        <nav
          className="ticket-pager"
          data-testid="ticket-pager"
          aria-label="Pagination"
        >
          <div className="pager-info">
            <span className="pager-count" data-testid="ticket-total-count">
              {total} {total === 1 ? "issue" : "issues"}
            </span>
            <label className="pager-size">
              <span>Per page</span>
              <select
                className="filter-select"
                data-testid="tickets-page-size"
                aria-label="Issues per page"
                value={pageSize}
                onChange={(e) => changePageSize(Number(e.target.value))}
              >
                {PAGE_SIZES.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
                <option value={0}>All</option>
              </select>
            </label>
          </div>
          <div className="pager-nav">
            <button
              type="button"
              className="secondary"
              data-testid="ticket-page-prev"
              disabled={safePage <= 1}
              onClick={() => setPage((p) => Math.max(1, p - 1))}
            >
              ‹ Prev
            </button>
            <span className="pager-status" data-testid="ticket-page-status">
              Page {safePage} of {totalPages}
            </span>
            <button
              type="button"
              className="secondary"
              data-testid="ticket-page-next"
              disabled={safePage >= totalPages}
              onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
            >
              Next ›
            </button>
          </div>
        </nav>
        </>
      )}
    </div>
  );
}

/* ── detail ─────────────────────────────────────────────────────── */

interface TicketDetailPanelProps {
  id: string;
  user: User | null;
  isAdmin: boolean;
  onLoadFdraft(bytes: ArrayBuffer): void;
  onBack(): void;
}

function TicketDetailPanel({
  id,
  user,
  isAdmin,
  onLoadFdraft,
  onBack,
}: TicketDetailPanelProps) {
  const [ticket, setTicket] = useState<TicketDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setTicket(await platformApi.getTicket(id));
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load this issue"));
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const isAuthor = !!user && !!ticket && ticket.author_id === user.id;
  // The author gets a safe subset of the state machine (ticket 066230d9,
  // mirrors server userMaySetState): close their ticket, park it to `backlog`
  // (recorded, do-not-work), un-park it, and reopen it. Everything else is
  // admin/PO-driven.
  const st = ticket?.state;
  const canClose =
    isAuthor &&
    (st === "triage" ||
      st === "needs_info" ||
      st === "declined" ||
      st === "backlog" ||
      st === "resolved");
  const canReopen = isAuthor && (st === "resolved" || st === "closed");
  const canBacklog = isAuthor && (st === "triage" || st === "needs_info");
  const canUnpark = isAuthor && st === "backlog";

  const setState = async (state: TicketState, failMsg: string) => {
    setBusy(true);
    setError(null);
    try {
      const updated = await platformApi.updateTicket(id, { state });
      setTicket(updated);
    } catch (err) {
      setError(accountErrorMessage(err, failMsg));
    } finally {
      setBusy(false);
    }
  };
  const close = () => setState("closed", "Could not close the issue");
  // Reopen a resolved ticket → in_progress ("the fix didn't stick"); a closed
  // one → triage (re-evaluate through the PO).
  const reopen = () =>
    setState(
      st === "closed" ? "triage" : "in_progress",
      "Could not reopen the issue",
    );
  const backlog = () => setState("backlog", "Could not move the issue to backlog");
  const unpark = () => setState("triage", "Could not take the issue out of backlog");

  if (loading) {
    return (
      <div className="fd-tickets" data-testid="ticket-detail">
        <DetailBar title="Issue" onBack={onBack} />
        <p className="empty" data-testid="detail-loading">
          Loading…
        </p>
      </div>
    );
  }

  if (error && !ticket) {
    return (
      <div className="fd-tickets" data-testid="ticket-detail">
        <DetailBar title="Issue" onBack={onBack} />
        <p className="error" role="alert" data-testid="detail-error">
          {error}
        </p>
      </div>
    );
  }

  if (!ticket) return null;

  return (
    <div className="fd-tickets" data-testid="ticket-detail">
      <DetailBar title="Issue" onBack={onBack} />

      {error && (
        <p className="error" role="alert" data-testid="detail-error">
          {error}
        </p>
      )}

      {editing ? (
        <TicketEdit
          ticket={ticket}
          onCancel={() => setEditing(false)}
          onSaved={(updated) => {
            setTicket(updated);
            setEditing(false);
          }}
        />
      ) : (
        <div className="detail-head">
          <div className="detail-head-row">
            <span className="detail-number" data-testid="detail-number">
              {ticketRef(ticket.number)}
            </span>
            <span
              className={`badge state-${stateBucket(ticket.state)}`}
              data-testid="detail-state"
            >
              {stateLabel(ticket.state)}
            </span>
            <h3 className="detail-title">{ticket.title}</h3>
          </div>
          <div className="detail-meta">
            <span>Opened {formatDateTime(ticket.created_at)}</span>
            <span>Updated {formatDateTime(ticket.updated_at)}</span>
          </div>
          <p className="detail-body" data-testid="detail-body">
            {linkify(ticket.body)}
          </p>
          {isAuthor && (
            <div className="detail-author-actions">
              <button
                type="button"
                className="secondary"
                data-testid="detail-edit"
                disabled={busy}
                onClick={() => setEditing(true)}
              >
                Edit
              </button>
              {canBacklog && (
                <button
                  type="button"
                  className="secondary"
                  data-testid="detail-backlog-ticket"
                  disabled={busy}
                  onClick={backlog}
                  title="Notiert, aber vom Agenten vorerst nicht bearbeiten"
                >
                  Move to backlog
                </button>
              )}
              {canUnpark && (
                <button
                  type="button"
                  className="secondary"
                  data-testid="detail-unpark-ticket"
                  disabled={busy}
                  onClick={unpark}
                  title="Zurück in Triage — der Agent darf es wieder bearbeiten"
                >
                  Out of backlog
                </button>
              )}
              {canClose && (
                <button
                  type="button"
                  className="secondary"
                  data-testid="detail-close-ticket"
                  disabled={busy}
                  onClick={close}
                >
                  Close issue
                </button>
              )}
              {canReopen && (
                <button
                  type="button"
                  className="secondary"
                  data-testid="detail-reopen-ticket"
                  disabled={busy}
                  onClick={reopen}
                  title="Ticket wieder öffnen"
                >
                  Reopen
                </button>
              )}
            </div>
          )}
        </div>
      )}

      <Artifacts
        ticketId={ticket.id}
        artifacts={ticket.artifacts}
        onLoadFdraft={onLoadFdraft}
        onError={setError}
      />

      <Thread
        comments={ticket.comments}
        isAdmin={isAdmin}
        canComment={!!user}
        onPosted={() => void reload()}
        ticketId={ticket.id}
      />
    </div>
  );
}

function DetailBar({ title, onBack }: { title: string; onBack(): void }) {
  return (
    <div className="tickets-bar">
      <button
        type="button"
        className="secondary back"
        data-testid="detail-back"
        onClick={onBack}
      >
        ← Issues
      </button>
      <h2 className="tickets-title">{title}</h2>
    </div>
  );
}

/* ── edit ───────────────────────────────────────────────────────── */

function TicketEdit({
  ticket,
  onCancel,
  onSaved,
}: {
  ticket: TicketDetail;
  onCancel(): void;
  onSaved(updated: TicketDetail): void;
}) {
  const [title, setTitle] = useState(ticket.title);
  const [body, setBody] = useState(ticket.body);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const canSave = title.trim().length > 0 && body.trim().length > 0 && !pending;

  const save = async () => {
    if (!canSave) return;
    setPending(true);
    setError(null);
    try {
      const updated = await platformApi.updateTicket(ticket.id, {
        title: title.trim(),
        body: body.trim(),
      });
      onSaved(updated);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not save your changes"));
      setPending(false);
    }
  };

  return (
    <div className="ticket-form" data-testid="ticket-edit">
      <label className="field">
        <span className="label">Title</span>
        <input
          className="text-input"
          data-testid="edit-title"
          value={title}
          disabled={pending}
          onChange={(e) => setTitle(e.target.value)}
        />
      </label>
      <label className="field">
        <span className="label">Description</span>
        <textarea
          className="textarea"
          data-testid="edit-body"
          value={body}
          rows={8}
          disabled={pending}
          onChange={(e) => setBody(e.target.value)}
        />
      </label>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="form-actions">
        <button
          type="button"
          className="secondary"
          disabled={pending}
          onClick={onCancel}
        >
          Cancel
        </button>
        <button
          type="button"
          className="primary"
          data-testid="edit-save"
          disabled={!canSave}
          onClick={save}
        >
          {pending ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}

/* ── artifacts ──────────────────────────────────────────────────── */

function Artifacts({
  ticketId,
  artifacts,
  onLoadFdraft,
  onError,
}: {
  ticketId: string;
  artifacts: TicketArtifact[];
  onLoadFdraft(bytes: ArrayBuffer): void;
  onError(msg: string): void;
}) {
  const [loadingFdraft, setLoadingFdraft] = useState(false);
  if (artifacts.length === 0) return null;

  // Ticket 582dfe: image ATTACHMENTS get the same preview-thumb treatment as
  // the window screenshot — only non-image files stay as download chips.
  const isImage = (a: TicketArtifact) =>
    a.kind === "screenshot" || a.mime.startsWith("image/");
  const screenshots = artifacts.filter(isImage);
  const fdraft = artifacts.find((a) => a.kind === "fdraft");
  const others = artifacts.filter((a) => !isImage(a) && a.kind !== "fdraft");

  const loadProject = async () => {
    if (!fdraft) return;
    setLoadingFdraft(true);
    try {
      const bytes = await platformApi.fetchArtifact(ticketId, fdraft.id);
      onLoadFdraft(bytes);
    } catch (err) {
      onError(accountErrorMessage(err, "Could not load the project"));
    } finally {
      setLoadingFdraft(false);
    }
  };

  return (
    <div className="artifacts" data-testid="ticket-artifacts">
      <h4 className="artifacts-title">Repro artifacts</h4>
      {screenshots.length > 0 && (
        <div className="screenshots">
          {screenshots.map((a) => (
            <a
              key={a.id}
              className="screenshot"
              href={artifactUrl(ticketId, a.id)}
              target="_blank"
              rel="noreferrer"
              data-testid={`artifact-screenshot-${a.id}`}
            >
              <img src={artifactUrl(ticketId, a.id)} alt={a.name} />
            </a>
          ))}
        </div>
      )}
      <div className="artifact-list">
        {fdraft && (
          <button
            type="button"
            className="primary load-project"
            data-testid="artifact-load-project"
            disabled={loadingFdraft}
            onClick={loadProject}
          >
            {loadingFdraft ? "Loading…" : "Load project"}
          </button>
        )}
        {others.map((a) => (
          <a
            key={a.id}
            className="artifact-chip"
            href={artifactUrl(ticketId, a.id)}
            target="_blank"
            rel="noreferrer"
            download={a.name}
            data-testid={`artifact-${a.id}`}
          >
            <span className="chip-kind">{a.kind}</span>
            <span className="chip-name">{a.name}</span>
          </a>
        ))}
      </div>
    </div>
  );
}

/* ── thread ─────────────────────────────────────────────────────── */

function Thread({
  ticketId,
  comments,
  isAdmin,
  canComment,
  onPosted,
}: {
  ticketId: string;
  comments: TicketComment[];
  isAdmin: boolean;
  canComment: boolean;
  onPosted(): void;
}) {
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Defense-in-depth: the backend already filters internal/withheld comments
  // for non-admins, but never render one we shouldn't.
  const visible = comments.filter((c) => commentVisible(c, isAdmin));

  const post = async () => {
    if (draft.trim().length === 0 || pending) return;
    setPending(true);
    setError(null);
    try {
      await platformApi.addComment(ticketId, draft.trim());
      setDraft("");
      onPosted();
    } catch (err) {
      setError(accountErrorMessage(err, "Could not post your comment"));
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="thread" data-testid="ticket-thread">
      <h4 className="thread-title">Discussion</h4>
      {visible.length === 0 ? (
        <p className="empty" data-testid="thread-empty">
          No comments yet.
        </p>
      ) : (
        <ul className="comments">
          {visible.map((c) => (
            <li
              key={c.id}
              className={`comment persona-${c.author_type}${c.internal ? " internal" : ""}`}
              data-testid={`comment-${c.id}`}
            >
              <span className="persona-tag" aria-hidden="true">
                {personaTag(c.author_type)}
              </span>
              <div className="comment-body">
                <div className="comment-head">
                  <span className="persona-name">
                    {personaLabel(c.author_type)}
                  </span>
                  {c.internal && (
                    <span className="internal-tag" title="Admin-only dev note">
                      internal
                    </span>
                  )}
                  <span className="comment-date">
                    {formatDateTime(c.created_at)}
                  </span>
                </div>
                <p className="comment-text">{linkify(c.body)}</p>
              </div>
            </li>
          ))}
        </ul>
      )}

      {canComment ? (
        <div className="comment-form">
          <textarea
            className="textarea"
            data-testid="comment-input"
            value={draft}
            rows={3}
            placeholder="Add a comment…"
            disabled={pending}
            onChange={(e) => setDraft(e.target.value)}
          />
          {error && (
            <p className="error" role="alert" data-testid="comment-error">
              {error}
            </p>
          )}
          <div className="form-actions">
            <button
              type="button"
              className="primary"
              data-testid="comment-submit"
              disabled={draft.trim().length === 0 || pending}
              onClick={post}
            >
              {pending ? "Posting…" : "Comment"}
            </button>
          </div>
        </div>
      ) : (
        <p className="empty" data-testid="comment-login-hint">
          Log in to join the discussion.
        </p>
      )}
    </div>
  );
}
