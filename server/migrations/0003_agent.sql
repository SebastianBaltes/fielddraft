-- WP6 Phase 1a — agent / work-order layer.
--
-- Per §5.1 / §9 / §10 of PLATFORM-ARCHITECTURE.md. The machine-to-machine API
-- that the two desktop agents (fielddraft-po, fielddraft-coding) drive: the
-- PO writes a work_order when it accepts a ticket; the coding agent claims it,
-- completes it, and records a deploy. Both tables are part of the durable
-- audit trail (§4.4).
--
-- deploys is created BEFORE work_orders because work_orders.deploy_id has an FK
-- into it.

CREATE TABLE IF NOT EXISTS deploys (
    id          CHAR(32)     NOT NULL,
    version     VARCHAR(64)  NOT NULL DEFAULT '',
    git_sha     VARCHAR(64)  NOT NULL DEFAULT '',
    ticket_ids  JSON         NULL,        -- array of ticket ids in this deploy
    status      ENUM('building','live','rolled_back') NOT NULL DEFAULT 'building',
    deployed_at DATETIME     NOT NULL,
    PRIMARY KEY (id),
    KEY idx_deploys_status (status),
    KEY idx_deploys_deployed_at (deployed_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS work_orders (
    ticket_id    CHAR(32) NOT NULL,       -- one work-order per ticket (PK = FK)
    payload      JSON     NOT NULL,        -- the §9 Work-Order JSON
    created_at   DATETIME NOT NULL,
    picked_up_at DATETIME NULL,            -- set when the coding agent claims it
    completed_at DATETIME NULL,            -- set when the coding agent completes it
    deploy_id    CHAR(32) NULL,            -- the deploy that resolved it
    PRIMARY KEY (ticket_id),
    KEY idx_work_orders_deploy (deploy_id),
    CONSTRAINT fk_work_orders_ticket FOREIGN KEY (ticket_id) REFERENCES tickets(id) ON DELETE CASCADE,
    CONSTRAINT fk_work_orders_deploy FOREIGN KEY (deploy_id) REFERENCES deploys(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
