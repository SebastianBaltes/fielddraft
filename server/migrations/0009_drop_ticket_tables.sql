-- Ticket #346 — drop the ticket tables. The ticket domain lives in IssueVault.
--
-- Migration 0008 cut the last constraint that still tied this database to the
-- tickets; the code stopped touching these tables in the same commit. This one
-- removes them for real, which is what the ticket asked for ("dortige Tickets
-- und entsprechenden Code entfernen").
--
-- BEFORE APPLYING TO AN INSTALLATION THAT STILL HAS ROWS: dump these tables.
-- The rows were migrated to IssueVault id-for-id (verified on the dev database:
-- all 24 tickets present in IssueVault under the same ids), so this destroys a
-- redundant copy, not the only one — but it is still a one-way door, and each
-- installation must check its own database before it runs.
--
--   mariadb-dump -u… -p… <db> tickets ticket_comments ticket_artifacts \
--       work_orders deploys > ticket-tables-backup.sql
--
-- The artifact BYTES are a separate matter: ticket_artifacts.blob_ref pointed
-- into the content-addressed blob store, and dropping the table makes those
-- blobs unreferenced (store_sql_gc.go no longer lists them). The blob GC is
-- opt-in, so nothing deletes them behind your back; the same artifacts already
-- live in IssueVault as files.
--
-- Children first — the foreign keys (fk_comments_ticket, fk_artifacts_ticket,
-- fk_work_orders_ticket, fk_work_orders_deploy) point at tickets/deploys.

DROP TABLE IF EXISTS ticket_comments;
DROP TABLE IF EXISTS ticket_artifacts;
DROP TABLE IF EXISTS work_orders;
DROP TABLE IF EXISTS tickets;
DROP TABLE IF EXISTS deploys;
