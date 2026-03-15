"""The 'compare' command: compare current build against a baseline."""

from pathlib import Path

import click
from rich.console import Console
from rich.table import Table

from ..config import WorkspaceConfig
from ..db import queries
from ..db.schema import init_db
from ..stats.comparison import compare_benchmark_results, compare_metric_values
from ._orchestrate import execute_run, register_environment, resolve_defaults

console = Console()


@click.command("compare")
@click.option(
    "--baseline",
    "-b",
    default=None,
    help="Baseline name to compare against (default: default baseline)",
)
@click.option(
    "--benchmarks", default=None, help="Comma-separated benchmark names (default: same as baseline)"
)
@click.option("--plan", "-p", default=None, help="GC plan")
@click.option(
    "--invocations",
    "-i",
    default=None,
    type=int,
    help="Number of invocations (default: from config)",
)
@click.option("--heap-multiplier", "-m", default=None, type=float, help="Heap size multiplier")
@click.option("--iterations", default=None, type=int, help="DaCapo timing iterations")
@click.option("--profile", default="release", help="Build profile")
@click.option(
    "--threshold", default=0.02, type=float, help="Threshold for significant change (default: 2%)"
)
@click.option(
    "--run-id", default=None, help="Compare an existing run instead of running new benchmarks"
)
@click.option(
    "--metric", default=None,
    help="Comma-separated metric names to compare (e.g. time.stw,time.other)",
)
@click.option("--db", default=None, type=click.Path(), help="Path to SQLite database")
@click.option("--note", "-n", default=None, help="Optional note to attach to this run")
def compare_cmd(
    baseline,
    benchmarks,
    plan,
    invocations,
    heap_multiplier,
    iterations,
    profile,
    threshold,
    run_id,
    metric,
    db,
    note,
):
    """Compare current build performance against a baseline.

    Runs benchmarks (or uses an existing run) and compares against a named
    baseline. Shows per-benchmark execution time differences.
    """
    ws = WorkspaceConfig.load()
    db_path = Path(db) if db else ws.db_path
    init_db(db_path)

    # Get baseline
    bl = queries.get_baseline(baseline, db_path)
    if bl is None:
        if baseline:
            console.print(f"[red]Baseline '{baseline}' not found.[/red]")
        else:
            console.print("[red]No default baseline set.[/red]")
            console.print("[dim]Set one with: mmtk-dev set-baseline <name>[/dim]")
        raise click.Abort()

    baseline_run = queries.get_run(bl["run_id"], db_path)
    if baseline_run is None:
        console.print(f"[red]Baseline '{bl['id']}' references a missing run.[/red]")
        raise click.Abort()
    baseline_build = queries.get_build(baseline_run["build_id"], db_path)
    baseline_results = queries.get_results_by_benchmark(bl["run_id"], db_path)

    if not baseline_results:
        console.print(f"[red]Baseline '{bl['id']}' has no results.[/red]")
        raise click.Abort()

    # Determine benchmarks to run
    if benchmarks:
        bm_list = [b.strip() for b in benchmarks.split(",")]
    else:
        # Same benchmarks as the baseline
        bm_list = sorted(baseline_results.keys())

    plan_resolved = plan or (baseline_build["gc_plan"] if baseline_build else ws.default_plan)
    plan_resolved, invocations, heap_multiplier, iterations = resolve_defaults(
        ws, plan=plan_resolved, invocations=invocations,
        heap_multiplier=heap_multiplier, iterations=iterations,
    )

    # Either use existing run or run new benchmarks
    if run_id:
        target_run = queries.get_run(run_id, db_path)
        if target_run is None:
            console.print(f"[red]Run '{run_id}' not found.[/red]")
            raise click.Abort()
    else:
        # Run new benchmarks
        console.print("\n[bold]Running benchmarks for comparison...[/bold]")
        console.print(
            f"  Baseline: [cyan]{bl['id']}[/cyan] (commit {baseline_build['core_commit'][:8] if baseline_build else '?'})"
        )
        console.print(f"  Plan: {plan_resolved}, Benchmarks: {', '.join(bm_list)}")
        console.print(f"  Invocations: {invocations}")
        console.print()

        build_id, testbed_id, _core_info, _binding_info = register_environment(
            ws, db_path, plan=plan_resolved, profile=profile,
        )

        # Check JDK exists
        jdk_path = ws.get_jdk_path(profile)
        if not jdk_path.exists():
            console.print(f"[red]Error: JDK not found at {jdk_path}[/red]")
            raise click.Abort()

        orch = execute_run(
            ws, db_path,
            build_id=build_id, testbed_id=testbed_id,
            benchmarks=bm_list, plan=plan_resolved, profile=profile,
            invocations=invocations, heap_multiplier=heap_multiplier,
            iterations=iterations,
            note=note,
        )

        if not orch.success:
            console.print("[red]Benchmark execution failed![/red]")
            raise click.Abort()

        run_id = orch.run_id

    # Now compare
    target_results = queries.get_results_by_benchmark(run_id, db_path)
    target_build = None
    target_run_data = queries.get_run(run_id, db_path)
    if target_run_data:
        target_build = queries.get_build(target_run_data["build_id"], db_path)

    _print_comparison(
        baseline_name=bl["id"],
        baseline_build=baseline_build,
        baseline_results=baseline_results,
        target_build=target_build,
        target_results=target_results,
        threshold=threshold,
    )

    # Print metric comparisons if requested
    if metric:
        if metric.strip().lower() == "all":
            metric_names = queries.list_available_metrics(bl["run_id"], db_path)
            if not metric_names:
                console.print("\n[yellow]⚠ No metrics available for this run.[/yellow]")
        else:
            metric_names = [m.strip() for m in metric.split(",")]
        for metric_name in metric_names:
            bl_metric_vals = queries.get_metric_values_by_benchmark(
                bl["run_id"], metric_name, db_path
            )
            tgt_metric_vals = queries.get_metric_values_by_benchmark(
                run_id, metric_name, db_path
            )
            if not bl_metric_vals and not tgt_metric_vals:
                console.print(
                    f"\n[yellow]⚠ No data for metric '{metric_name}' in either run.[/yellow]"
                )
                # Show available metrics
                available = queries.list_available_metrics(bl["run_id"], db_path)
                if available:
                    console.print(
                        f"  Available metrics: {', '.join(available)}"
                    )
                continue
            _print_metric_comparison(
                metric_name=metric_name,
                baseline_values=bl_metric_vals,
                target_values=tgt_metric_vals,
                threshold=threshold,
            )


def _print_comparison(
    baseline_name: str,
    baseline_build: dict | None,
    baseline_results: dict,
    target_build: dict | None,
    target_results: dict,
    threshold: float,
):
    """Print a comparison table between baseline and target results."""
    bl_commit = baseline_build["core_commit"][:8] if baseline_build else "?"
    tgt_plan = target_build["gc_plan"] if target_build else "?"

    console.print(
        f'\n[bold]Comparing against baseline "{baseline_name}"[/bold] '
        f"(commit {bl_commit}, {tgt_plan})"
    )
    console.print()

    result = compare_benchmark_results(baseline_results, target_results, threshold)

    table = Table()
    table.add_column("Benchmark", style="bold")
    table.add_column("Baseline (ms)", justify="right")
    table.add_column("Current (ms)", justify="right")
    table.add_column("Diff", justify="right")
    table.add_column("Status", justify="center")

    for c in result["comparisons"]:
        bl_stats = c["baseline"]
        tgt_stats = c["target"]
        diff = c["diff"]
        change = c["change"]

        if bl_stats["mean"] is not None and tgt_stats["mean"] is not None:
            bl_str = f"{bl_stats['mean']:.1f} ±{bl_stats['ci']:.1f}"
            tgt_str = f"{tgt_stats['mean']:.1f} ±{tgt_stats['ci']:.1f}"
            diff_str = f"{diff:+.2%}"

            if change == "faster":
                status = "[green]✅ faster[/green]"
                diff_str = f"[green]{diff_str}[/green]"
            elif change == "slower":
                status = "[red]❌ slower[/red]"
                diff_str = f"[red]{diff_str}[/red]"
            else:
                status = "➡️  neutral"
        elif bl_stats["mean"] is None:
            bl_str = "[dim]no data[/dim]"
            tgt_str = f"{tgt_stats['mean']:.1f}" if tgt_stats["mean"] else "-"
            diff_str = "-"
            status = "[yellow]⚠ no baseline[/yellow]"
        else:
            bl_str = f"{bl_stats['mean']:.1f}"
            tgt_str = "[dim]no data[/dim]"
            diff_str = "-"
            status = "[yellow]⚠ no result[/yellow]"

        table.add_row(c["benchmark"], bl_str, tgt_str, diff_str, status)

    console.print(table)

    # Geometric mean
    geomean = result["geomean_diff"]
    geomean_change = result["geomean_change"]
    if geomean != 0.0:
        geomean_str = f"{geomean:+.2%}"
        if geomean_change == "faster":
            console.print(f"\nGeometric mean: [green]{geomean_str} (improvement)[/green]")
        elif geomean_change == "slower":
            console.print(f"\nGeometric mean: [red]{geomean_str} (regression)[/red]")
        else:
            console.print(f"\nGeometric mean: {geomean_str} (neutral)")


def _print_metric_comparison(
    metric_name: str,
    baseline_values: dict[str, list[float]],
    target_values: dict[str, list[float]],
    threshold: float,
):
    """Print a comparison table for a specific metric."""
    result = compare_metric_values(baseline_values, target_values, metric_name, threshold)

    console.print(f"\n[bold]Metric: {metric_name}[/bold]")

    # Determine unit suffix based on metric name
    unit = "ms" if metric_name.startswith("time.") else ""
    unit_label = f" ({unit})" if unit else ""

    table = Table()
    table.add_column("Benchmark", style="bold")
    table.add_column(f"Baseline{unit_label}", justify="right")
    table.add_column(f"Current{unit_label}", justify="right")
    table.add_column("Diff", justify="right")
    table.add_column("Status", justify="center")

    for c in result["comparisons"]:
        bl_stats = c["baseline"]
        tgt_stats = c["target"]
        diff = c["diff"]
        change = c["change"]

        if bl_stats["mean"] is not None and tgt_stats["mean"] is not None:
            bl_str = f"{bl_stats['mean']:.1f} ±{bl_stats['ci']:.1f}"
            tgt_str = f"{tgt_stats['mean']:.1f} ±{tgt_stats['ci']:.1f}"
            diff_str = f"{diff:+.2%}"

            if change == "faster":
                status = "[green]✅ faster[/green]"
                diff_str = f"[green]{diff_str}[/green]"
            elif change == "slower":
                status = "[red]❌ slower[/red]"
                diff_str = f"[red]{diff_str}[/red]"
            else:
                status = "➡️  neutral"
        elif bl_stats["mean"] is None:
            bl_str = "[dim]no data[/dim]"
            tgt_str = f"{tgt_stats['mean']:.1f}" if tgt_stats["mean"] else "-"
            diff_str = "-"
            status = "[yellow]⚠ no baseline[/yellow]"
        else:
            bl_str = f"{bl_stats['mean']:.1f}"
            tgt_str = "[dim]no data[/dim]"
            diff_str = "-"
            status = "[yellow]⚠ no result[/yellow]"

        table.add_row(c["benchmark"], bl_str, tgt_str, diff_str, status)

    console.print(table)

    geomean = result["geomean_diff"]
    geomean_change = result["geomean_change"]
    if geomean != 0.0:
        geomean_str = f"{geomean:+.2%}"
        if geomean_change == "faster":
            console.print(f"  Geometric mean: [green]{geomean_str} (improvement)[/green]")
        elif geomean_change == "slower":
            console.print(f"  Geometric mean: [red]{geomean_str} (regression)[/red]")
        else:
            console.print(f"  Geometric mean: {geomean_str} (neutral)")

