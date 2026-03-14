"""The 'compare' command: compare current build against a baseline."""

from pathlib import Path

import click
from rich.console import Console
from rich.table import Table

from ..config import WorkspaceConfig, detect_testbed, get_git_info
from ..db import queries
from ..db.schema import init_db
from ..runner.parser import results_to_db_format
from ..runner.runner import LocalRunner, RunConfig
from ..stats.analysis import (
    classify_change,
    compute_diff,
    compute_geomean_ratio,
    compute_statistics,
)

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
@click.option("--db", default=None, type=click.Path(), help="Path to SQLite database")
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
    db,
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
    baseline_build = queries.get_build(baseline_run["build_id"], db_path) if baseline_run else None
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

    plan = plan or (baseline_build["gc_plan"] if baseline_build else ws.default_plan)
    invocations = invocations or ws.default_invocations
    heap_multiplier = heap_multiplier if heap_multiplier is not None else ws.default_heap_multiplier
    iterations = iterations or ws.default_iterations

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
        console.print(f"  Plan: {plan}, Benchmarks: {', '.join(bm_list)}")
        console.print(f"  Invocations: {invocations}")
        console.print()

        # Get git info and register build
        core_info = get_git_info(ws.mmtk_core)
        binding_info = get_git_info(ws.mmtk_openjdk)

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
            db_path=db_path,
        )

        jdk_path = ws.get_jdk_path(profile)
        if not jdk_path.exists():
            console.print(f"[red]Error: JDK not found at {jdk_path}[/red]")
            raise click.Abort()

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
        )

        result = runner.run_benchmarks(run_config)
        db_results = results_to_db_format(result.results)
        queries.insert_results(run_id, db_results, db_path)
        queries.complete_run(run_id, "completed", db_path)

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

    table = Table()
    table.add_column("Benchmark", style="bold")
    table.add_column("Baseline (ms)", justify="right")
    table.add_column("Current (ms)", justify="right")
    table.add_column("Diff", justify="right")
    table.add_column("Status", justify="center")

    all_diffs = []

    # Compare all benchmarks that exist in both
    all_benchmarks = sorted(set(list(baseline_results.keys()) + list(target_results.keys())))

    for bm in all_benchmarks:
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

        if bl_stats["mean"] is not None and tgt_stats["mean"] is not None:
            diff = compute_diff(bl_stats["mean"], tgt_stats["mean"])
            all_diffs.append(diff)
            change = classify_change(diff, threshold)

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

        table.add_row(bm, bl_str, tgt_str, diff_str, status)

    console.print(table)

    # Geometric mean
    if all_diffs:
        geomean = compute_geomean_ratio(all_diffs)
        change = classify_change(geomean, threshold)
        geomean_str = f"{geomean:+.2%}"
        if change == "faster":
            console.print(f"\nGeometric mean: [green]{geomean_str} (improvement)[/green]")
        elif change == "slower":
            console.print(f"\nGeometric mean: [red]{geomean_str} (regression)[/red]")
        else:
            console.print(f"\nGeometric mean: {geomean_str} (neutral)")
