"""FastAPI REST API for mmtk-dev."""

import os
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, HTTPException, Query
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse
from fastapi.staticfiles import StaticFiles

from ..db import queries
from ..db.schema import init_db
from ..stats.analysis import (
    classify_change,
    compute_diff,
    compute_geomean_ratio,
    compute_statistics,
)


def _db() -> Path | None:
    """Get the database path from environment or default."""
    env = os.environ.get("MMTK_DEV_DB_PATH")
    return Path(env) if env else None


@asynccontextmanager
async def lifespan(_app: FastAPI) -> AsyncIterator[None]:
    init_db(_db())
    yield


app = FastAPI(title="MMTk Dev API", version="0.1.0", lifespan=lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


# ── Builds ───────────────────────────────────────────────────────────────────


@app.get("/api/builds")
def list_builds(limit: int = Query(50, ge=1, le=500)):
    return queries.list_builds(_db(), limit=limit)


@app.get("/api/builds/{build_id}")
def get_build(build_id: str):
    build = queries.get_build(build_id, _db())
    if build is None:
        raise HTTPException(404, "Build not found")
    return build


# ── Runs ─────────────────────────────────────────────────────────────────────


@app.get("/api/runs")
def list_runs(
    limit: int = Query(50, ge=1, le=500),
    build_id: str | None = None,
    testbed_id: str | None = None,
):
    return queries.list_runs(_db(), limit=limit, build_id=build_id, testbed_id=testbed_id)


@app.get("/api/runs/{run_id}")
def get_run(run_id: str):
    run = queries.get_run(run_id, _db())
    if run is None:
        raise HTTPException(404, "Run not found")
    return run


@app.get("/api/runs/{run_id}/results")
def get_run_results(run_id: str):
    run = queries.get_run(run_id, _db())
    if run is None:
        raise HTTPException(404, "Run not found")

    grouped = queries.get_results_by_benchmark(run_id, _db())
    summary = {}
    for bm, results in grouped.items():
        times = [r["execution_time_ms"] for r in results if r["execution_time_ms"] is not None]
        stats = compute_statistics(times)
        summary[bm] = {
            "stats": stats,
            "results": results,
        }
    return summary


# ── Baselines ────────────────────────────────────────────────────────────────


@app.get("/api/baselines")
def list_baselines():
    return queries.list_baselines(_db())


@app.get("/api/baselines/{name}")
def get_baseline(name: str):
    bl = queries.get_baseline(name, _db())
    if bl is None:
        raise HTTPException(404, "Baseline not found")
    return bl


# ── Compare ──────────────────────────────────────────────────────────────────


@app.get("/api/compare")
def compare_runs(
    baseline: str | None = None,
    run_id: str | None = None,
    threshold: float = 0.02,
):
    """Compare a run against a baseline."""
    # Get baseline
    bl = queries.get_baseline(baseline, _db())
    if bl is None:
        raise HTTPException(404, "Baseline not found")

    baseline_results = queries.get_results_by_benchmark(bl["run_id"], _db())

    # Get target run (use latest if not specified)
    if run_id is None:
        target_run = queries.get_latest_run(_db())
        if target_run is None:
            raise HTTPException(404, "No runs found")
        run_id = target_run["id"]

    target_results = queries.get_results_by_benchmark(run_id, _db())

    comparisons = []
    all_diffs = []

    all_bms = sorted(set(list(baseline_results.keys()) + list(target_results.keys())))

    for bm in all_bms:
        bl_times = [
            r["execution_time_ms"]
            for r in baseline_results.get(bm, [])
            if r["execution_time_ms"] is not None
        ]
        tgt_times = [
            r["execution_time_ms"]
            for r in target_results.get(bm, [])
            if r["execution_time_ms"] is not None
        ]

        bl_stats = compute_statistics(bl_times)
        tgt_stats = compute_statistics(tgt_times)

        entry: dict = {"benchmark": bm, "baseline": bl_stats, "target": tgt_stats}

        if bl_stats["mean"] is not None and tgt_stats["mean"] is not None:
            diff = compute_diff(bl_stats["mean"], tgt_stats["mean"])
            all_diffs.append(diff)
            entry["diff"] = diff
            entry["change"] = classify_change(diff, threshold)
        else:
            entry["diff"] = None
            entry["change"] = "no_data"

        comparisons.append(entry)

    geomean = compute_geomean_ratio(all_diffs) if all_diffs else 0.0

    return {
        "baseline_name": bl["id"],
        "baseline_run_id": bl["run_id"],
        "target_run_id": run_id,
        "comparisons": comparisons,
        "geomean_diff": geomean,
        "geomean_change": classify_change(geomean, threshold),
    }


# ── Testbeds ─────────────────────────────────────────────────────────────────


@app.get("/api/testbeds")
def list_testbeds():
    return queries.list_testbeds(_db())


# ── Health ───────────────────────────────────────────────────────────────────


@app.get("/api/health")
def health():
    return {"status": "ok", "version": "0.1.0"}


# ── Static Files & Dashboard ─────────────────────────────────────────────────

_DIST_DIR = Path(__file__).parent.parent / "web" / "dist"


@app.get("/", response_class=HTMLResponse)
def dashboard() -> str:
    """Serve the web dashboard."""
    index_html = _DIST_DIR / "index.html"
    if not index_html.exists():
        return "<h1>Frontend not built</h1><p>Run <code>mmtk-dev server</code> or <code>cd frontend && npm run build</code></p>"
    return index_html.read_text()


# Mount static after the root route so it doesn't shadow it
if _DIST_DIR.exists():
    app.mount("/static", StaticFiles(directory=str(_DIST_DIR)), name="static")
