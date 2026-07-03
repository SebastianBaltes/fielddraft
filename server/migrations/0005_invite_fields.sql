-- WP12 — invite fields: replace the free-form `note` with the intended
-- recipient (name + email) plus an invitation message.
--
-- Per §5.1 / invite flow of PLATFORM-ARCHITECTURE.md. No backward compat: the
-- prod invites table is empty, so dropping `note` is safe. New columns default
-- to '' / NULL so an admin may create an invite without specifying a recipient.
ALTER TABLE invites
    DROP COLUMN IF EXISTS note,
    ADD COLUMN IF NOT EXISTS invitee_name  VARCHAR(255) NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS invitee_email VARCHAR(320) NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS message       TEXT;
