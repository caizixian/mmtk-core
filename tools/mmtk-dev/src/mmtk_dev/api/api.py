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
from ..stats.analysis import compute_statistics
from ..stats.comparison import compare_benchmark_results, compare_metric_values


def _db() -> Path | None:
    """Get the database path from environment or default."""
    env = os.environ.get("MMTK_DEV_DB_PATH")
    return Path(env) if env else None


@asynccontextmanager
async def lifespan(application: FastAPI) -> AsyncIterator[None]:
    init_db(_db())
    # Mount static files at startup so it works even if web/dist/ is
    # created after the module is first imported (e.g. by `server --skip-build`
    # followed by a manual `npm run build`).
    if _DIST_DIR.exists():
        application.mount("/static", StaticFiles(directory=str(_DIST_DIR)), name="static")
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
    metrics_by_result = queries.get_metrics_by_result_id(run_id, _db())

    summary = {}
    for bm, results in grouped.items():
        times = [r["execution_time_ms"] for r in results if r["execution_time_ms"] is not None]
        stats = compute_statistics(times)
        # Attach metrics to each result
        for r in results:
            r["metrics"] = metrics_by_result.get(r["id"], [])
        summary[bm] = {
            "stats": stats,
            "results": results,
        }
    return summary


@app.get("/api/runs/{run_id}/metrics")
def get_run_metrics(run_id: str):
    """List available metric names for a run."""
    run = queries.get_run(run_id, _db())
    if run is None:
        raise HTTPException(404, "Run not found")
    return queries.list_available_metrics(run_id, _db())


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
    metric: str | None = None,
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

    result = compare_benchmark_results(baseline_results, target_results, threshold)
    result["baseline_name"] = bl["id"]
    result["baseline_run_id"] = bl["run_id"]
    result["target_run_id"] = run_id

    # If a metric is specified, add metric comparison(s)
    if metric:
        if metric.lower() == "all":
            # Compare all available metrics
            all_metric_names = queries.list_available_metrics(bl["run_id"], _db())
            metric_comparisons = []
            for m_name in all_metric_names:
                bl_vals = queries.get_metric_values_by_benchmark(bl["run_id"], m_name, _db())
                tgt_vals = queries.get_metric_values_by_benchmark(run_id, m_name, _db())
                if bl_vals or tgt_vals:
                    mc = compare_metric_values(bl_vals, tgt_vals, m_name, threshold)
                    metric_comparisons.append(mc)
            result["metric_comparisons"] = metric_comparisons
        else:
            bl_metric_vals = queries.get_metric_values_by_benchmark(
                bl["run_id"], metric, _db()
            )
            tgt_metric_vals = queries.get_metric_values_by_benchmark(run_id, metric, _db())
            mc = compare_metric_values(
                bl_metric_vals, tgt_metric_vals, metric, threshold
            )
            result["metric_comparisons"] = [mc]

    return result


# ── Testbeds ─────────────────────────────────────────────────────────────────


@app.get("/api/testbeds")
def list_testbeds():
    return queries.list_testbeds(_db())


# ── Trends ───────────────────────────────────────────────────────────────────


@app.get("/api/trends")
def get_trends(
    limit: int = Query(20, ge=1, le=100),
    metric: str | None = None,
):
    """Batch endpoint: per-benchmark trends across recent runs.

    Without 'metric', returns execution time trends.
    With 'metric=time.stw', returns trends for that specific metric.
    """
    if metric:
        return queries.get_metric_trends(_db(), metric_name=metric, limit=limit)
    return queries.get_trends(_db(), limit=limit)


# ── Health ───────────────────────────────────────────────────────────────────


@app.get("/api/health")
def health():
    return {"status": "ok", "version": "0.1.0"}


# ── Static Files & Dashboard ─────────────────────────────────────────────────

_DIST_DIR = Path(__file__).resolve().parent.parent.parent.parent / "frontend" / "dist"


@app.get("/{path:path}", response_class=HTMLResponse)
def spa_fallback(path: str):
    """Serve static files or fall back to index.html for react-router."""
    # Serve actual static files if they exist (JS, CSS, etc.)
    if path.startswith("static/"):
        file_path = _DIST_DIR / path.removeprefix("static/")
        if file_path.exists() and file_path.is_file():
            from fastapi.responses import FileResponse

            return FileResponse(file_path)

    index_html = _DIST_DIR / "index.html"
    if not index_html.exists():
        return "<h1>Frontend not built</h1><p>Run <code>mmtk-dev server</code> or <code>cd frontend &amp;&amp; npm run build</code></p>"
    return index_html.read_text()


# Static files are mounted during the lifespan startup event above
# so that it works even if web/dist/ didn't exist at import time.

