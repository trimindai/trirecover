-- Per-session schema. One database file per scan session.

CREATE TABLE IF NOT EXISTS session (
    id           TEXT PRIMARY KEY,
    drive_path   TEXT NOT NULL,
    strategy     TEXT NOT NULL,
    opened_at    DATETIME NOT NULL,
    reopened_at  DATETIME
);

CREATE TABLE IF NOT EXISTS progress (
    session_id           TEXT NOT NULL,
    state                TEXT NOT NULL,
    sectors_scanned      INTEGER NOT NULL,
    sectors_total        INTEGER NOT NULL,
    files_found          INTEGER NOT NULL,
    bytes_recoverable    INTEGER NOT NULL,
    current_phase        TEXT NOT NULL,
    bad_sectors_skipped  INTEGER NOT NULL,
    ts                   DATETIME NOT NULL,
    FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_progress_session_ts
    ON progress(session_id, ts DESC);

CREATE TABLE IF NOT EXISTS file (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    path            TEXT NOT NULL,
    kind            TEXT NOT NULL,
    size_bytes      INTEGER NOT NULL,
    modified        DATETIME,
    recoverability  INTEGER NOT NULL,
    head_hex        TEXT NOT NULL,
    source_json     TEXT NOT NULL,
    found_at        DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_file_kind ON file(kind);
CREATE INDEX IF NOT EXISTS idx_file_size ON file(size_bytes);
CREATE INDEX IF NOT EXISTS idx_file_name ON file(name);

-- Bad sectors encountered during the scan, kept so a re-scan can skip them.
CREATE TABLE IF NOT EXISTS bad_sector (
    lba       INTEGER PRIMARY KEY,
    last_seen DATETIME NOT NULL
);

-- Free-form key/value for extensions (probability metrics, integrity hashes).
CREATE TABLE IF NOT EXISTS kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
