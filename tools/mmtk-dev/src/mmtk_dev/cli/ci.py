"""The 'ci' command: CI integration mode for regression detection."""

import sys
from pathlib import Path

import click
from rich.console import Console

from ..config import ALL_DACAPO_2006, WorkspaceConfig, detect_testbed, get_git_info
from ..db import queries
from ..db.schema import init_db
from ..runner.parser import results_to_db_format
from ..runner.runner import LocalRunner, RunConfig
from ..stats.comparison import compare_benchmark_results

console = Console()


@click.command("ci")
@click.option("--core-commit", default=None, help="mmtk-core commit SHA")
@click.option("--core-branch", default=None, help="mmtk-core branch name")
@click.option("--binding-commit", default=None, help="mmtk-openjdk commit SHA")
@click.option("--plans", default="GenImmix", help="Comma-separated GC plans to benchmark")
@click.option("--benchmarks", "-b", default=None, help="Comma-separated benchmark names or 'all'")
@click.option("--invocations", "-i", default=20, type=int, help="Number of invocations")
@click.option("--heap-multiplier", "-m", default=3.0, type=float, help="Heap size multiplier")
@click.option("--iterations", default=6, type=int, help="DaCapo timing iterations")
@click.option(
    "--alert-threshold",
    default=0.02,
    type=float,
    help="Threshold for regression alert (default: 2%)",
)
@click.option(
    "--output-format", default="rich", type=click.Choice(["rich", "markdown"]), help="Output format"
)
@click.option("--profile", default="release", help="Build profile")
@click.option("--db", default=None, type=click.Path(), help="Path to database")
def ci_cmd(
    core_commit,
    core_branch,
    binding_commit,
    plans,
    benchmarks,
    invocations,
    heap_multiplier,
    iterations,
    alert_threshold,
    output_format,
    profile,
    db,
):
    """Run benchmarks in CI mode with regression detection.

    Runs benchmarks, compares against the most recent completed run for the
    same plan on the same testbed, and outputs a report. Exits with non-zero
    code if regressions are detected.
    """
    ws = WorkspaceConfig.load()
    db_path = Path(db) if db else ws.db_path
    init_db(db_path)

    plan_list = [p.strip() for p in plans.split(",")]

    if benchmarks is None:
        bm_list = ws.default_benchmarks
    elif benchmarks == "all":
        bm_list = ALL_DACAPO_2006
    else:
        bm_list = [b.strip() for b in benchmarks.split(",")]

    # Get git info (use provided values or auto-detect)
    core_info = get_git_info(ws.mmtk_core)
    if core_commit:
        core_info["commit"] = core_commit
    if core_branch:
        core_info["branch"] = core_branch

    binding_info = get_git_info(ws.mmtk_openjdk)
    if binding_commit:
        binding_info["commit"] = binding_commit

    # Detect testbed
    tb = detect_testbed()
    testbed_id = ws.testbed_id or str(tb.get("id", "local"))
    queries.ensure_testbed(
        testbed_id,
        ws.testbed_name or str(tb.get("name", "local")),
        cpu_model=str(tb.get("cpu_model", "")),
        cpu_cores=int(tb.get("cpu_cores", 0)),
        memory_gb=float(tb.get("memory_gb", 0)),
        db_path=db_path,
    )

    has_regression = False
    all_reports: list[str] = []

    for plan in plan_list:
        if output_format == "markdown":
            all_reports.append(f"## {plan}\n")
        else:
            console.print(f"\n[bold]═══ {plan} ═══[/bold]")

        # Register build and create run
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

        run_id = queries.create_run(
            build_id=build_id,
            testbed_id=testbed_id,
            invocations=invocations,
            heap_multiplier=heap_multiplier,
            metadata={"ci": True},
            db_path=db_path,
        )

        # Execute benchmarks
        jdk_path = ws.get_jdk_path(profile)
        if not jdk_path.exists():
            msg = f"JDK not found at {jdk_path}"
            if output_format == "markdown":
                all_reports.append(f"**Error**: {msg}\n")
            else:
                console.print(f"[red]{msg}[/red]")
            continue

        runner = LocalRunner(ws)
        run_config = RunConfig(
            benchmarks=bm_list,
            plan=plan,
            jdk_path=jdk_path,
            invocations=invocations,
            heap_multiplier=heap_multiplier,
            iterations=iterations,
            suite=ws.dacapo_suite,
            dacapo_jar=ws.dacapo_jar,
            probes_path=ws.probes_path,
        )

        result = runner.run_benchmarks(run_config)
        db_results = results_to_db_format(result.results)
        queries.insert_results(run_id, db_results, db_path)
        queries.complete_run(run_id, "completed", db_path)

        # Find baseline to compare against (default baseline or most recent run)
        bl = queries.get_baseline(db_path=db_path)
        if bl:
            baseline_results = queries.get_results_by_benchmark(bl["run_id"], db_path)
            baseline_label = f"baseline '{bl['id']}'"
        else:
            baseline_results = {}
            baseline_label = "no baseline"

        target_results = queries.get_results_by_benchmark(run_id, db_path)

        # Generate report
        comparison = compare_benchmark_results(
            baseline_results, target_results, alert_threshold
        )
        regression_found = _generate_ci_report(
            plan,
            baseline_label,
            comparison,
            output_format,
            all_reports,
        )
        if regression_found:
            has_regression = True

    # Output markdown if requested
    if output_format == "markdown":
        print("\n".join(all_reports))

    # Exit with non-zero if regressions found
    if has_regression:
        sys.exit(1)


def _generate_ci_report(
    plan: str,
    baseline_label: str,
    result: dict,
    output_format: str,
    reports: list[str],
) -> bool:
    """Generate a CI comparison report. Returns True if regressions detected."""
    regression_found = False
    rows = []

    for c in result["comparisons"]:
        bl_stats = c["baseline"]
        tgt_stats = c["target"]
        diff = c["diff"]
        change = c["change"]

        if bl_stats["mean"] is not None and tgt_stats["mean"] is not None:
            if change == "slower":
                regression_found = True
                status = "❌ regression"
            elif change == "faster":
                status = "✅ faster"
            else:
                status = "➡️ neutral"

            rows.append(
                {
                    "bm": c["benchmark"],
                    "bl_mean": f"{bl_stats['mean']:.1f} ±{bl_stats['ci']:.1f}",
                    "tgt_mean": f"{tgt_stats['mean']:.1f} ±{tgt_stats['ci']:.1f}",
                    "diff": f"{diff:+.2%}",
                    "status": status,
                }
            )

    if output_format == "markdown":
        reports.append(f"Compared against {baseline_label}\n")
        reports.append("| Benchmark | Baseline (ms) | Current (ms) | Diff | Status |")
        reports.append("|-----------|--------------|-------------|------|--------|")
        for r in rows:
            emoji = (
                "🟥" if "regression" in r["status"] else ("🟩" if "faster" in r["status"] else "")
            )
            reports.append(
                f"| {r['bm']} | {r['bl_mean']} | {r['tgt_mean']} | {r['diff']} {emoji} | {r['status']} |"
            )

        geomean = result["geomean_diff"]
        if geomean != 0.0:
            reports.append(f"\n**Geometric mean**: {geomean:+.2%}")
        reports.append("")
    else:
        from rich.table import Table

        table = Table(title=f"Compared against {baseline_label}")
        table.add_column("Benchmark", style="bold")
        table.add_column("Baseline (ms)", justify="right")
        table.add_column("Current (ms)", justify="right")
        table.add_column("Diff", justify="right")
        table.add_column("Status")

        for r in rows:
            table.add_row(r["bm"], r["bl_mean"], r["tgt_mean"], r["diff"], r["status"])
        console.print(table)

        geomean = result["geomean_diff"]
        if geomean != 0.0:
            console.print(f"Geometric mean: {geomean:+.2%}")

    return regression_found
