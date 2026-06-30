-- WP1 Phase 0a — initial schema.
--
-- Per §5.1 of PLATFORM-ARCHITECTURE.md. Ticket tables (tickets,
-- ticket_comments, ticket_artifacts, work_orders, deploys) are WP2 and are
-- intentionally NOT created here.
--
-- Large/binary payloads (project documents, snapshot documents, thumbnails)
-- are content-addressed blob files on disk; rows carry only the sha256 ref.

CREATE TABLE users (
    id            CHAR(32)     NOT NULL,
    email         VARCHAR(320) NOT NULL,
    display_name  VARCHAR(255) NOT NULL DEFAULT '',
    password_hash VARBINARY(64) NOT NULL,
    salt          VARBINARY(32) NOT NULL,
    role          ENUM('user','admin') NOT NULL DEFAULT 'user',
    created_at    DATETIME     NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_users_email (email)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE invites (
    token      CHAR(32)     NOT NULL,
    created_by CHAR(32)     NOT NULL,
    note       VARCHAR(512) NOT NULL DEFAULT '',
    expires_at DATETIME     NOT NULL,
    used_by    CHAR(32)     NULL,
    used_at    DATETIME     NULL,
    created_at DATETIME     NOT NULL,
    PRIMARY KEY (token),
    KEY idx_invites_created_by (created_by),
    CONSTRAINT fk_invites_created_by FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT fk_invites_used_by   FOREIGN KEY (used_by)   REFERENCES users(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE sessions (
    token      CHAR(32) NOT NULL,
    user_id    CHAR(32) NOT NULL,
    expires_at DATETIME NOT NULL,
    PRIMARY KEY (token),
    KEY idx_sessions_user (user_id),
    CONSTRAINT fk_sessions_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE reset_tokens (
    token      CHAR(32) NOT NULL,
    user_id    CHAR(32) NOT NULL,
    expires_at DATETIME NOT NULL,
    PRIMARY KEY (token),
    KEY idx_reset_user (user_id),
    CONSTRAINT fk_reset_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE projects (
    id            CHAR(32)     NOT NULL,
    owner_id      CHAR(32)     NOT NULL,
    name          VARCHAR(255) NOT NULL DEFAULT 'Untitled',
    visibility    ENUM('private','public') NOT NULL DEFAULT 'private',
    document_ref  CHAR(64)     NULL,   -- sha256 hex of the document blob
    thumbnail_ref CHAR(64)     NULL,   -- sha256 hex of the thumbnail blob
    created_at    DATETIME     NOT NULL,
    updated_at    DATETIME     NOT NULL,
    PRIMARY KEY (id),
    KEY idx_projects_owner (owner_id),
    KEY idx_projects_visibility (visibility),
    CONSTRAINT fk_projects_owner FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE snapshots (
    id           CHAR(32) NOT NULL,
    project_id   CHAR(32) NOT NULL,
    document_ref CHAR(64) NULL,   -- sha256 hex of the snapshot document blob
    created_at   DATETIME NOT NULL,
    PRIMARY KEY (id),
    KEY idx_snapshots_project (project_id),
    CONSTRAINT fk_snapshots_project FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
