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

import { useCallback, useEffect, useState } from "react";
import {
  platformApi,
  artifactUrl,
  type TicketArtifact,
  type TicketCategory,
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
  categoryLabel,
  commentVisible,
  formatDate,
  personaLabel,
  personaTag,
  stateBucket,
  stateLabel,
} from "./ticketView";

interface TicketsViewProps {
  /** The logged-in user (null while guest — listing needs a session). */
  user: User | null;
  isAdmin: boolean;
  /** Load a ticket's `.fdraft` artifact bytes into the live workspace as an
   *  unbound copy (App.tsx wires this to the kernel store). */
  onLoadFdraft(bytes: ArrayBuffer): void;
  /** Deep-link: open this ticket id on mount (e.g. straight from a report). */
  initialTicketId?: string | null;
}

const CATEGORIES: TicketCategory[] = ["bug", "feature", "question", "other"];

const STATES: TicketState[] = [
  "triage",
  "needs_info",
  "accepted",
  "in_progress",
  "deploying",
  "resolved",
  "closed",
  "declined",
  "rejected_spam",
  "rejected_malicious",
];

export function TicketsView({
  user,
  isAdmin,
  onLoadFdraft,
  initialTicketId,
}: TicketsViewProps) {
  // Master/detail mode: the list or a single ticket id. There is no create
  // mode — issues are filed via the 👤 "Report Issue…" flow.
  const [mode, setMode] = useState<
    { kind: "list" } | { kind: "detail"; id: string }
  >(
    initialTicketId
      ? { kind: "detail", id: initialTicketId }
      : { kind: "list" },
  );

  if (mode.kind === "detail") {
    return (
      <TicketDetailPanel
        id={mode.id}
        user={user}
        isAdmin={isAdmin}
        onLoadFdraft={onLoadFdraft}
        onBack={() => setMode({ kind: "list" })}
      />
    );
  }

  return (
    <TicketList user={user} onOpen={(id) => setMode({ kind: "detail", id })} />
  );
}

/* ── list ───────────────────────────────────────────────────────── */

interface TicketListProps {
  user: User | null;
  onOpen(id: string): void;
}

function TicketList({ user, onOpen }: TicketListProps) {
  const [tickets, setTickets] = useState<TicketSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [onlyMine, setOnlyMine] = useState(false);
  const [stateFilter, setStateFilter] = useState<TicketState | "">("");
  const [categoryFilter, setCategoryFilter] = useState<TicketCategory | "">("");
  const [query, setQuery] = useState("");

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    const filter: TicketListFilter = {
      author: onlyMine ? "me" : undefined,
      state: stateFilter || undefined,
      category: categoryFilter || undefined,
      q: query || undefined,
    };
    try {
      const list = await platformApi.listTickets(filter);
      // Most-recent activity first (created falls back via updated).
      list.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      setTickets(list);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load tickets"));
    } finally {
      setLoading(false);
    }
  }, [onlyMine, stateFilter, categoryFilter, query]);

  useEffect(() => {
    void reload();
  }, [reload]);

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
        <select
          className="filter-select"
          data-testid="tickets-category-filter"
          aria-label="Filter by category"
          value={categoryFilter}
          onChange={(e) =>
            setCategoryFilter(e.target.value as TicketCategory | "")
          }
        >
          <option value="">All categories</option>
          {CATEGORIES.map((c) => (
            <option key={c} value={c}>
              {categoryLabel(c)}
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
        <ul className="ticket-rows" data-testid="ticket-list">
          {tickets.map((t) => (
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
                <span
                  className={`badge state-${stateBucket(t.state)}`}
                  data-testid={`ticket-state-${t.id}`}
                >
                  {stateLabel(t.state)}
                </span>
                <span className="row-title" title={t.title}>
                  {t.title}
                </span>
                <span className="row-category">
                  {categoryLabel(t.category)}
                </span>
                <span className="row-meta">
                  Updated {formatDate(t.updated_at)}
                </span>
              </button>
            </li>
          ))}
        </ul>
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
  // The state machine only lets a non-admin author close a resolved ticket
  // (resolved → closed). Other transitions are admin/PO-driven.
  const canClose = isAuthor && ticket?.state === "resolved";

  const close = async () => {
    setBusy(true);
    setError(null);
    try {
      const updated = await platformApi.updateTicket(id, { state: "closed" });
      setTicket(updated);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not close the issue"));
    } finally {
      setBusy(false);
    }
  };

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
            <span
              className={`badge state-${stateBucket(ticket.state)}`}
              data-testid="detail-state"
            >
              {stateLabel(ticket.state)}
            </span>
            <h3 className="detail-title">{ticket.title}</h3>
          </div>
          <div className="detail-meta">
            <span>{categoryLabel(ticket.category)}</span>
            <span>Opened {formatDate(ticket.created_at)}</span>
            <span>Updated {formatDate(ticket.updated_at)}</span>
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
                    {formatDate(c.created_at)}
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
