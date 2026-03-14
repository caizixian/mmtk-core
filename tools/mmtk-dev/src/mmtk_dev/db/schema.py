"""SQLite database schema and connection management."""

import sqlite3
from pathlib import Path
from contextlib import contextmanager

DEFAULT_DB_PATH = Path.home() / ".mmtk-dev" / "mmtk-dev.db"

SCHEMA = """
CREATE TABLE IF NOT EXISTS testbed (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    cpu_model   TEXT,
    cpu_cores   INTEGER,
    memory_gb   REAL,
    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS build (
    id              TEXT PRIMARY KEY,
    core_repo       TEXT NOT NULL,
    core_commit     TEXT NOT NULL,
    core_branch     TEXT,
    binding_repo    TEXT NOT NULL,
    binding_commit  TEXT NOT NULL,
    binding_branch  TEXT,
    gc_plan         TEXT NOT NULL,
    build_profile   TEXT NOT NULL,
    rust_toolchain  TEXT,
    features        TEXT,
    jdk_path        TEXT,
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS run (
    id              TEXT PRIMARY KEY,
    build_id        TEXT NOT NULL REFERENCES build(id),
    testbed_id      TEXT NOT NULL REFERENCES testbed(id),
    running_ng_id   TEXT,
    invocations     INTEGER NOT NULL,
    heap_multiplier REAL,
    started_at      TIMESTAMP NOT NULL,
    finished_at     TIMESTAMP,
    status          TEXT NOT NULL DEFAULT 'running',
    metadata        TEXT
);

CREATE TABLE IF NOT EXISTS result (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id              TEXT NOT NULL REFERENCES run(id),
    benchmark           TEXT NOT NULL,
    suite               TEXT NOT NULL,
    heap_size_mb        INTEGER,
    heap_factor         REAL,
    invocation          INTEGER NOT NULL,
    execution_time_ms   REAL,
    status              TEXT NOT NULL,
    UNIQUE(run_id, benchmark, suite, heap_factor, invocation)
);

CREATE TABLE IF NOT EXISTS metric (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    result_id   INTEGER NOT NULL REFERENCES result(id),
    name        TEXT NOT NULL,
    value       REAL NOT NULL,
    UNIQUE(result_id, name)
);

CREATE TABLE IF NOT EXISTS baseline (
    id          TEXT PRIMARY KEY,
    run_id      TEXT NOT NULL REFERENCES run(id),
    is_default  BOOLEAN DEFAULT 0,
    description TEXT,
    created_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_result_run_id ON result(run_id);
CREATE INDEX IF NOT EXISTS idx_result_benchmark ON result(benchmark);
CREATE INDEX IF NOT EXISTS idx_metric_result_id ON metric(result_id);
CREATE INDEX IF NOT EXISTS idx_run_build_id ON run(build_id);
CREATE INDEX IF NOT EXISTS idx_run_testbed_id ON run(testbed_id);
"""


def get_db_path() -> Path:
    """Get the database path, creating parent dirs if needed."""
    db_path = DEFAULT_DB_PATH
    db_path.parent.mkdir(parents=True, exist_ok=True)
    return db_path


def init_db(db_path: Path | None = None) -> None:
    """Initialize the database with the schema."""
    if db_path is None:
        db_path = get_db_path()
    conn = sqlite3.connect(str(db_path))
    conn.executescript(SCHEMA)
    conn.close()


@contextmanager
def get_connection(db_path: Path | None = None):
    """Get a database connection as a context manager."""
    if db_path is None:
        db_path = get_db_path()
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA foreign_keys=ON")
    try:
        yield conn
        conn.commit()
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.close()
