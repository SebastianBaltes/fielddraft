-- WP2 Phase 0b — DB-backed ticket system (GitHub-Issues-like).
--
-- Per §5.1 / §7 of PLATFORM-ARCHITECTURE.md. Replaces the file-blob ticket
-- flow (tickets/open/<id>/) with authored, stateful issues plus a comment
-- thread and repro artifacts. work_orders/deploys are WP6 and NOT created here.
--
-- Large/binary repro payloads (screenshots, .fdraft ZIPs, attachments) are
-- content-addressed blob files on disk (the same sha256 mechanism WP1 uses for
-- documents/thumbnails); ticket_artifacts rows carry only the ref + metadata.

CREATE TABLE tickets (
    id            CHAR(32)     NOT NULL,
    author_id     CHAR(32)     NOT NULL,
    title         VARCHAR(255) NOT NULL DEFAULT '',
    body          TEXT         NOT NULL,
    state         ENUM('triage','needs_info','accepted','in_progress','deploying',
                       'resolved','closed','declined','rejected_spam','rejected_malicious')
                  NOT NULL DEFAULT 'triage',
    category      ENUM('bug','feature','question','other') NULL,
    po_spam       BOOL         NOT NULL DEFAULT 0,
    po_malicious  BOOL         NOT NULL DEFAULT 0,
    po_vision_fit ENUM('fit','partial','off') NULL,
    po_reason     TEXT         NULL,
    po_verdict_at DATETIME     NULL,
    created_at    DATETIME     NOT NULL,
    updated_at    DATETIME     NOT NULL,
    PRIMARY KEY (id),
    KEY idx_tickets_author (author_id),
    KEY idx_tickets_state (state),
    KEY idx_tickets_category (category),
    CONSTRAINT fk_tickets_author FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE ticket_comments (
    id          CHAR(32) NOT NULL,
    ticket_id   CHAR(32) NOT NULL,
    author_type ENUM('user','po','dev','system') NOT NULL,
    author_id   CHAR(32) NULL,   -- NULL for po/dev/system
    internal    BOOL     NOT NULL DEFAULT 0,   -- dev notes: admin + agents only, never user
    body        TEXT     NOT NULL,
    created_at  DATETIME NOT NULL,
    withheld    BOOL     NOT NULL DEFAULT 0,   -- leak-filter hold (§4.2)
    PRIMARY KEY (id),
    KEY idx_comments_ticket (ticket_id),
    CONSTRAINT fk_comments_ticket FOREIGN KEY (ticket_id) REFERENCES tickets(id) ON DELETE CASCADE,
    CONSTRAINT fk_comments_author FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE ticket_artifacts (
    id         CHAR(32) NOT NULL,
    ticket_id  CHAR(32) NOT NULL,
    kind       ENUM('screenshot','fdraft','actions','settings','ui_state','attachment') NOT NULL,
    blob_ref   CHAR(64) NOT NULL,   -- sha256 hex of the artifact blob
    mime       VARCHAR(255) NOT NULL DEFAULT '',
    size       BIGINT   NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL,
    name       VARCHAR(255) NOT NULL DEFAULT '',
    PRIMARY KEY (id),
    KEY idx_artifacts_ticket (ticket_id),
    CONSTRAINT fk_artifacts_ticket FOREIGN KEY (ticket_id) REFERENCES tickets(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
