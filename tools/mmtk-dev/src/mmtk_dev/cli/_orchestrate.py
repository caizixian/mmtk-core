"""Shared CLI orchestration: benchmark resolution, git/testbed, build/run registration, execution."""

from dataclasses import dataclass
from pathlib import Path

from ..config import ALL_DACAPO, WorkspaceConfig
from ..db import queries
from ..environment import detect_testbed, get_git_info
from ..runner.parser import BenchmarkResult, results_to_db_format, results_to_metrics_format
from ..runner.runner import LocalRunner, RunConfig


@dataclass
class OrchestrateResult:
    """Result of a full orchestration: run + store."""

    run_id: str
    build_id: str
    testbed_id: str
    results: list[BenchmarkResult]
    success: bool


def resolve_benchmarks(
    benchmarks_arg: str | None,
    ws: WorkspaceConfig,
) -> list[str]:
    """Resolve the --benchmarks CLI arg into a list of benchmark names."""
    if benchmarks_arg is None:
        return ws.default_benchmarks
    if benchmarks_arg == "all":
        return ALL_DACAPO
    return [b.strip() for b in benchmarks_arg.split(",")]


def resolve_defaults(
    ws: WorkspaceConfig,
    *,
    plan: str | None = None,
    invocations: int | None = None,
    heap_multiplier: float | None = None,
    iterations: int | None = None,
) -> tuple[str, int, float, int]:
    """Resolve CLI args with config defaults. Returns (plan, invocations, heap_mul, iterations)."""
    return (
        plan or ws.default_plan,
        invocations or ws.default_invocations,
        heap_multiplier if heap_multiplier is not None else ws.default_heap_multiplier,
        iterations or ws.default_iterations,
    )


def register_environment(
    ws: WorkspaceConfig,
    db_path: Path | None,
    *,
    plan: str,
    profile: str,
    core_commit: str | None = None,
    core_branch: str | None = None,
    binding_commit: str | None = None,
) -> tuple[str, str, dict[str, str], dict[str, str]]:
    """Detect git/testbed, register build. Returns (build_id, testbed_id, core_info, binding_info)."""
    core_info = get_git_info(ws.mmtk_core)
    if core_commit:
        core_info["commit"] = core_commit
    if core_branch:
        core_info["branch"] = core_branch

    binding_info = get_git_info(ws.mmtk_openjdk)
    if binding_commit:
        binding_info["commit"] = binding_commit

    tb = detect_testbed()
    testbed_id = ws.testbed_id or str(tb.get("id", "local"))
    testbed_name = ws.testbed_name or str(tb.get("name", "local"))
    queries.ensure_testbed(
        testbed_id,
        testbed_name,
        cpu_model=str(tb.get("cpu_model", "")),
        cpu_cores=int(tb.get("cpu_cores", 0)),
        memory_gb=float(tb.get("memory_gb", 0)),
        db_path=db_path,
    )

    build_id = queries.register_build(
        core_repo=core_info.get("repo", "unknown"),
        core_commit=core_info.get("commit", "unknown"),
        core_branch=core_info.get("branch"),
        binding_repo=binding_info.get("repo", "unknown"),
        binding_commit=binding_info.get("commit", "unknown"),
        binding_branch=binding_info.get("branch"),
        gc_plan=plan,
        build_profile=profile,
        jdk_path=str(ws.get_jdk_path(profile)),
        db_path=db_path,
    )

    return build_id, testbed_id, core_info, binding_info


def execute_run(
    ws: WorkspaceConfig,
    db_path: Path | None,
    *,
    build_id: str,
    testbed_id: str,
    benchmarks: list[str],
    plan: str,
    profile: str,
    invocations: int,
    heap_multiplier: float,
    iterations: int,
    log_dir: Path | None = None,
    metadata: dict | None = None,
    store_metrics: bool = True,
) -> OrchestrateResult:
    """Create a run, execute benchmarks, store results. Returns the orchestration result."""
    run_id = queries.create_run(
        build_id=build_id,
        testbed_id=testbed_id,
        invocations=invocations,
        heap_multiplier=heap_multiplier,
        metadata=metadata,
        db_path=db_path,
    )

    jdk_path = ws.get_jdk_path(profile)

    runner = LocalRunner(ws)
    run_config = RunConfig(
        benchmarks=benchmarks,
        plan=plan,
        jdk_path=jdk_path,
        invocations=invocations,
        heap_multiplier=heap_multiplier,
        iterations=iterations,
        suite="dacapochopin",
        dacapo_jar=ws.dacapo_jar,
        log_dir=log_dir,
        probes_path=ws.probes_path,
    )

    result = runner.run_benchmarks(run_config)

    if not result.success and not result.results:
        queries.complete_run(run_id, "failed", db_path)
        return OrchestrateResult(
            run_id=run_id,
            build_id=build_id,
            testbed_id=testbed_id,
            results=[],
            success=False,
        )

    # Store results
    db_results = results_to_db_format(result.results)
    queries.insert_results(run_id, db_results, db_path)

    # Store MMTk metrics (if any were parsed from logs)
    if store_metrics:
        metrics_data = results_to_metrics_format(result.results)
        if metrics_data:
            queries.insert_metrics_for_run(run_id, metrics_data, db_path)

    queries.complete_run(run_id, "completed", db_path)

    # Track running-ng run ID
    if result.run_id:
        queries.update_running_ng_id(run_id, result.run_id, db_path)

    return OrchestrateResult(
        run_id=run_id,
        build_id=build_id,
        testbed_id=testbed_id,
        results=result.results,
        success=True,
    )
