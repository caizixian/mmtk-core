"""The 'ci' command: CI integration mode for regression detection."""

import sys
from pathlib import Path

import click
from rich.console import Console

from ..config import WorkspaceConfig
from ..db import queries
from ..db.schema import init_db
from ..stats.comparison import compare_benchmark_results
from ._orchestrate import execute_run, register_environment, resolve_benchmarks

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
    bm_list = resolve_benchmarks(benchmarks, ws)

    has_regression = False
    all_reports: list[str] = []

    for plan in plan_list:
        if output_format == "markdown":
            all_reports.append(f"## {plan}\n")
        else:
            console.print(f"\n[bold]═══ {plan} ═══[/bold]")

        build_id, testbed_id, _core_info, _binding_info = register_environment(
            ws, db_path, plan=plan, profile=profile,
            core_commit=core_commit, core_branch=core_branch,
            binding_commit=binding_commit,
        )

        # Check JDK exists
        jdk_path = ws.get_jdk_path(profile)
        if not jdk_path.exists():
            msg = f"JDK not found at {jdk_path}"
            if output_format == "markdown":
                all_reports.append(f"**Error**: {msg}\n")
            else:
                console.print(f"[red]{msg}[/red]")
            continue

        orch = execute_run(
            ws, db_path,
            build_id=build_id, testbed_id=testbed_id,
            benchmarks=bm_list, plan=plan, profile=profile,
            invocations=invocations, heap_multiplier=heap_multiplier,
            iterations=iterations, metadata={"ci": True},
            store_metrics=False,
        )

        # Find baseline to compare against (default baseline or most recent run)
        bl = queries.get_baseline(db_path=db_path)
        if bl:
            baseline_results = queries.get_results_by_benchmark(bl["run_id"], db_path)
            baseline_label = f"baseline '{bl['id']}'"
        else:
            baseline_results = {}
            baseline_label = "no baseline"

        target_results = queries.get_results_by_benchmark(orch.run_id, db_path)

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
