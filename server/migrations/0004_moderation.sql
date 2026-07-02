-- WP8 — moderation: per-comment PO review, real admin alerts, user ban.
--
-- Per §4.2 (leak filter / classification), §4.4 (admin-awareness + post-hoc
-- admin controls: "User/Invite sperren") and §8 (PO classifies spam/malicious)
-- of PLATFORM-ARCHITECTURE.md. Three coherent gaps closed together:
--
--   B1  ticket_comments.po_reviewed — the PO post-moderates EVERY user comment.
--       New user comments start unreviewed (0); po/dev/system comments are
--       inherently reviewed (inserted with 1).
--   B2  admin_alerts — real admin-notify (not just a log line) for malicious
--       tickets/comments, withheld outgoing comments, and user bans.
--   B3  users.disabled — a banned user can neither log in nor use a session.

-- B1 — per-comment PO review flag. Backfill: every existing comment counts as
-- already reviewed (the PO sweep only needs to see NEW user comments).
ALTER TABLE ticket_comments
    ADD COLUMN IF NOT EXISTS po_reviewed BOOL NOT NULL DEFAULT 1 AFTER withheld;

-- B3 — user disable flag.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS disabled BOOL NOT NULL DEFAULT 0 AFTER role;

-- B2 — admin alert feed.
CREATE TABLE IF NOT EXISTS admin_alerts (
    id         CHAR(32) NOT NULL,
    kind       ENUM('malicious_ticket','malicious_comment','withheld_comment','user_banned') NOT NULL,
    ticket_id  CHAR(32) NULL,
    detail     TEXT     NOT NULL,
    created_at DATETIME NOT NULL,
    seen       BOOL     NOT NULL DEFAULT 0,
    PRIMARY KEY (id),
    KEY idx_admin_alerts_seen (seen),
    KEY idx_admin_alerts_created_at (created_at),
    KEY idx_admin_alerts_ticket (ticket_id),
    CONSTRAINT fk_admin_alerts_ticket FOREIGN KEY (ticket_id) REFERENCES tickets(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
