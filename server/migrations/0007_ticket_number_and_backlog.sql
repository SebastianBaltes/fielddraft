-- Ticket 066230d9 — short human/agent-friendly reference + the `backlog` state.
--
-- (1) A per-DB auto-incrementing ticket `number`, displayed as "#N". The opaque
--     CHAR(32) `id` STAYS the storage/routing key (comments, artifacts, deploys
--     all FK to it); `number` is purely an additional short reference. In prod
--     the IssueVault backend supplies its own per-project number instead — this
--     column only backs the self-hosted MariaDB store. Existing rows are
--     backfilled in creation order; the WHERE-guard makes a re-run after a
--     partial failure converge (idempotent DDL, see migrations.go header).
--
-- (2) The `backlog` state: "recorded, but the agent should not work it" — a
--     user-set parking state outside the triage→…→resolved flow.

ALTER TABLE tickets ADD COLUMN IF NOT EXISTS number INT NULL UNIQUE;

UPDATE tickets t
  JOIN (SELECT id, ROW_NUMBER() OVER (ORDER BY created_at, id) AS rn FROM tickets) x
    ON t.id = x.id
  SET t.number = x.rn
  WHERE t.number IS NULL;

ALTER TABLE tickets MODIFY number INT NOT NULL AUTO_INCREMENT;

ALTER TABLE tickets MODIFY COLUMN state
    ENUM('triage','needs_info','accepted','in_progress','deploying',
         'resolved','closed','declined','backlog','rejected_spam','rejected_malicious')
    NOT NULL DEFAULT 'triage';
