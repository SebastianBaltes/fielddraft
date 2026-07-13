-- Ticket #346 — the ticket domain lives in IssueVault, not in this database.
--
-- The MariaDB and file-backed ticket stores are gone (selectTicketStore now
-- returns IssueVault, full stop). That leaves ONE thing that must change in the
-- schema, and it is not cosmetic:
--
--   admin_alerts.ticket_id carried a FOREIGN KEY into `tickets`. The moderation
--   path still raises alerts that reference a ticket (agent.go: malicious
--   ticket / malicious comment / withheld comment) — but the id it now passes is
--   an ISSUEVAULT ticket id, for which no row in `tickets` exists and never will.
--   The FK rejects every such insert, so exactly the alerts an admin most needs
--   to see would be dropped. Cut the constraint; keep the column (and its index)
--   as a plain reference into IssueVault.
--
-- The ticket TABLES themselves (tickets, ticket_comments, ticket_artifacts,
-- work_orders, deploys) are deliberately left in place here: their rows were
-- migrated to IssueVault, but dropping them destroys the only local copy, and a
-- migration is the wrong place to make that irreversible call silently. They are
-- unreferenced from code as of this ticket — a follow-up may drop them.

-- MariaDB has no "DROP FOREIGN KEY IF EXISTS"; a fresh database created after
-- this migration never had the constraint, so tolerate its absence.
SET @fk_exists := (
    SELECT COUNT(*)
    FROM information_schema.TABLE_CONSTRAINTS
    WHERE CONSTRAINT_SCHEMA = DATABASE()
      AND TABLE_NAME = 'admin_alerts'
      AND CONSTRAINT_NAME = 'fk_admin_alerts_ticket'
);
SET @sql := IF(@fk_exists > 0,
    'ALTER TABLE admin_alerts DROP FOREIGN KEY fk_admin_alerts_ticket',
    'DO 0');
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
