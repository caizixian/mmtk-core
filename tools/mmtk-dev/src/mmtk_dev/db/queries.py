"""Database query functions."""

import hashlib
import json
import uuid
from datetime import UTC, datetime
from pathlib import Path

from .schema import get_connection

# ── Testbed ──────────────────────────────────────────────────────────────────


def ensure_testbed(
    testbed_id: str,
    name: str,
    cpu_model: str | None = None,
    cpu_cores: int | None = None,
    memory_gb: float | None = None,
    db_path: Path | None = None,
) -> str:
    """Create or update a testbed. Returns the testbed ID."""
    with get_connection(db_path) as conn:
        conn.execute(
            """INSERT INTO testbed (id, name, cpu_model, cpu_cores, memory_gb)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                 name=excluded.name, cpu_model=excluded.cpu_model,
                 cpu_cores=excluded.cpu_cores, memory_gb=excluded.memory_gb""",
            (testbed_id, name, cpu_model, cpu_cores, memory_gb),
        )
    return testbed_id


def list_testbeds(db_path: Path | None = None) -> list[dict]:
    with get_connection(db_path) as conn:
        rows = conn.execute("SELECT * FROM testbed ORDER BY created_at DESC").fetchall()
        return [dict(r) for r in rows]


# ── Build ────────────────────────────────────────────────────────────────────


def make_build_id(core_commit: str, binding_commit: str, gc_plan: str, features: str = "") -> str:
    """Deterministic build ID from build parameters."""
    key = f"{core_commit}:{binding_commit}:{gc_plan}:{features}"
    return hashlib.sha256(key.encode()).hexdigest()[:16]


def register_build(
    core_repo: str,
    core_commit: str,
    binding_repo: str,
    binding_commit: str,
    gc_plan: str,
    build_profile: str,
    core_branch: str | None = None,
    binding_branch: str | None = None,
    rust_toolchain: str | None = None,
    features: str | None = None,
    jdk_path: str | None = None,
    db_path: Path | None = None,
) -> str:
    """Register a build. Returns the build ID."""
    build_id = make_build_id(core_commit, binding_commit, gc_plan, features or "")
    with get_connection(db_path) as conn:
        conn.execute(
            """INSERT INTO build (id, core_repo, core_commit, core_branch,
                 binding_repo, binding_commit, binding_branch,
                 gc_plan, build_profile, rust_toolchain, features, jdk_path)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET jdk_path=excluded.jdk_path""",
            (
                build_id,
                core_repo,
                core_commit,
                core_branch,
                binding_repo,
                binding_commit,
                binding_branch,
                gc_plan,
                build_profile,
                rust_toolchain,
                features,
                jdk_path,
            ),
        )
    return build_id


def get_build(build_id: str, db_path: Path | None = None) -> dict | None:
    with get_connection(db_path) as conn:
        row = conn.execute("SELECT * FROM build WHERE id = ?", (build_id,)).fetchone()
        return dict(row) if row else None


def list_builds(db_path: Path | None = None, limit: int = 50) -> list[dict]:
    with get_connection(db_path) as conn:
        rows = conn.execute(
            "SELECT * FROM build ORDER BY created_at DESC LIMIT ?", (limit,)
        ).fetchall()
        return [dict(r) for r in rows]


# ── Run ──────────────────────────────────────────────────────────────────────


def create_run(
    build_id: str,
    testbed_id: str,
    invocations: int,
    heap_multiplier: float | None = None,
    running_ng_id: str | None = None,
    metadata: dict | None = None,
    db_path: Path | None = None,
) -> str:
    """Create a new run. Returns the run ID."""
    run_id = uuid.uuid4().hex[:12]
    now = datetime.now(UTC).isoformat()
    with get_connection(db_path) as conn:
        conn.execute(
            """INSERT INTO run (id, build_id, testbed_id, running_ng_id,
                 invocations, heap_multiplier, started_at, status, metadata)
               VALUES (?, ?, ?, ?, ?, ?, ?, 'running', ?)""",
            (
                run_id,
                build_id,
                testbed_id,
                running_ng_id,
                invocations,
                heap_multiplier,
                now,
                json.dumps(metadata) if metadata else None,
            ),
        )
    return run_id


def complete_run(run_id: str, status: str = "completed", db_path: Path | None = None) -> None:
    now = datetime.now(UTC).isoformat()
    with get_connection(db_path) as conn:
        conn.execute(
            "UPDATE run SET status = ?, finished_at = ? WHERE id = ?",
            (status, now, run_id),
        )


def update_running_ng_id(
    run_id: str, running_ng_id: str, db_path: Path | None = None
) -> None:
    """Set the running-ng run identifier for a run."""
    with get_connection(db_path) as conn:
        conn.execute(
            "UPDATE run SET running_ng_id = ? WHERE id = ?",
            (running_ng_id, run_id),
        )


def get_run(run_id: str, db_path: Path | None = None) -> dict | None:
    with get_connection(db_path) as conn:
        row = conn.execute("SELECT * FROM run WHERE id = ?", (run_id,)).fetchone()
        return dict(row) if row else None


def list_runs(
    db_path: Path | None = None,
    limit: int = 50,
    build_id: str | None = None,
    testbed_id: str | None = None,
) -> list[dict]:
    with get_connection(db_path) as conn:
        query = "SELECT * FROM run WHERE 1=1"
        params: list = []
        if build_id:
            query += " AND build_id = ?"
            params.append(build_id)
        if testbed_id:
            query += " AND testbed_id = ?"
            params.append(testbed_id)
        query += " ORDER BY started_at DESC LIMIT ?"
        params.append(limit)
        rows = conn.execute(query, params).fetchall()
        return [dict(r) for r in rows]


def get_latest_run(db_path: Path | None = None) -> dict | None:
    """Get the most recently started run."""
    with get_connection(db_path) as conn:
        row = conn.execute("SELECT * FROM run ORDER BY started_at DESC LIMIT 1").fetchone()
        return dict(row) if row else None


# ── Result ───────────────────────────────────────────────────────────────────


def insert_results(run_id: str, results: list[dict], db_path: Path | None = None) -> None:
    """Batch-insert results for a run.

    Each result dict should have: benchmark, suite, heap_size_mb, heap_factor,
    invocation, execution_time_ms, status.
    """
    with get_connection(db_path) as conn:
        conn.executemany(
            """INSERT OR REPLACE INTO result
               (run_id, benchmark, suite, heap_size_mb, heap_factor,
                invocation, execution_time_ms, status)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
            [
                (
                    run_id,
                    r["benchmark"],
                    r["suite"],
                    r.get("heap_size_mb"),
                    r.get("heap_factor"),
                    r["invocation"],
                    r.get("execution_time_ms"),
                    r["status"],
                )
                for r in results
            ],
        )


def get_results(run_id: str, db_path: Path | None = None) -> list[dict]:
    with get_connection(db_path) as conn:
        rows = conn.execute(
            "SELECT * FROM result WHERE run_id = ? ORDER BY benchmark, invocation",
            (run_id,),
        ).fetchall()
        return [dict(r) for r in rows]


def get_results_by_benchmark(run_id: str, db_path: Path | None = None) -> dict[str, list[dict]]:
    """Get results grouped by benchmark name."""
    results = get_results(run_id, db_path)
    grouped: dict[str, list[dict]] = {}
    for r in results:
        bm = r["benchmark"]
        if bm not in grouped:
            grouped[bm] = []
        grouped[bm].append(r)
    return grouped


# ── Metric ───────────────────────────────────────────────────────────────────


def insert_metrics(result_id: int, metrics: dict[str, float], db_path: Path | None = None) -> None:
    """Insert metrics for a result."""
    with get_connection(db_path) as conn:
        conn.executemany(
            """INSERT OR REPLACE INTO metric (result_id, name, value)
               VALUES (?, ?, ?)""",
            [(result_id, name, value) for name, value in metrics.items()],
        )


def insert_metrics_for_run(
    run_id: str,
    metrics_by_benchmark: dict[str, list[dict[str, float]]],
    db_path: Path | None = None,
) -> None:
    """Insert MMTk metrics for a run, matching them to stored result rows.

    Args:
        run_id: the run these metrics belong to.
        metrics_by_benchmark: mapping of benchmark name → list of per-invocation
            metric dicts (one dict per invocation, in order).
    """
    with get_connection(db_path) as conn:
        for benchmark, inv_metrics_list in metrics_by_benchmark.items():
            # Get result rows for this benchmark, ordered by invocation
            rows = conn.execute(
                """SELECT id, invocation FROM result
                   WHERE run_id = ? AND benchmark = ?
                   ORDER BY invocation""",
                (run_id, benchmark),
            ).fetchall()

            for row, metrics in zip(rows, inv_metrics_list, strict=False):
                if metrics:
                    conn.executemany(
                        """INSERT OR REPLACE INTO metric (result_id, name, value)
                           VALUES (?, ?, ?)""",
                        [(row["id"], name, value) for name, value in metrics.items()],
                    )


# ── Baseline ─────────────────────────────────────────────────────────────────


def set_baseline(
    name: str,
    run_id: str,
    is_default: bool = True,
    description: str | None = None,
    db_path: Path | None = None,
) -> None:
    """Create or update a named baseline."""
    with get_connection(db_path) as conn:
        if is_default:
            # Clear existing defaults
            conn.execute("UPDATE baseline SET is_default = 0 WHERE is_default = 1")
        conn.execute(
            """INSERT INTO baseline (id, run_id, is_default, description)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                 run_id=excluded.run_id, is_default=excluded.is_default,
                 description=excluded.description""",
            (name, run_id, is_default, description),
        )


def get_baseline(name: str | None = None, db_path: Path | None = None) -> dict | None:
    """Get a baseline by name. If name is None, get the default baseline."""
    with get_connection(db_path) as conn:
        if name:
            row = conn.execute("SELECT * FROM baseline WHERE id = ?", (name,)).fetchone()
        else:
            row = conn.execute("SELECT * FROM baseline WHERE is_default = 1").fetchone()
        return dict(row) if row else None


def list_baselines(db_path: Path | None = None) -> list[dict]:
    with get_connection(db_path) as conn:
        rows = conn.execute("SELECT * FROM baseline ORDER BY created_at DESC").fetchall()
        return [dict(r) for r in rows]
