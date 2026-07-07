-- Server-side CGAL meshing (docs/CGAL-SERVER-MESHING-PLAN.md §P4): the
-- persistent job queue. Jobs survive restarts; the boot recovery resets
-- running → queued. The STL result is a content-addressed blob (blob_ref,
-- same store as project documents/artifacts).
CREATE TABLE IF NOT EXISTS mesh_jobs (
    id          VARCHAR(64)  PRIMARY KEY,
    owner_id    VARCHAR(64)  NOT NULL,
    project_id  VARCHAR(64)  NOT NULL,
    part_id     VARCHAR(64)  NOT NULL,
    params_json TEXT         NOT NULL,
    status      VARCHAR(16)  NOT NULL,
    blob_ref    VARCHAR(80)  NOT NULL DEFAULT '',
    report_json TEXT,
    error       TEXT,
    created_at  DATETIME(3)  NOT NULL,
    started_at  DATETIME(3)  NULL,
    finished_at DATETIME(3)  NULL,
    INDEX idx_mesh_jobs_claim (status, created_at),
    INDEX idx_mesh_jobs_owner (owner_id, status)
);
