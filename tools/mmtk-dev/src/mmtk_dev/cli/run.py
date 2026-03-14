"""The 'run' command: execute benchmarks and store results."""

from pathlib import Path

import click
from rich.console import Console
from rich.table import Table

from ..config import ALL_DACAPO_2006, WorkspaceConfig, detect_testbed, get_git_info
from ..db import queries
from ..db.schema import init_db
from ..runner.parser import results_to_db_format
from ..runner.runner import LocalRunner, RunConfig
from ..stats.analysis import compute_statistics

console = Console()


@click.command("run")
@click.option(
    "--benchmarks",
    "-b",
    default=None,
    help="Comma-separated benchmark names, or 'all' for all DaCapo 2006",
)
@click.option("--plan", "-p", default=None, help="GC plan (e.g. GenImmix)")
@click.option("--invocations", "-i", default=None, type=int, help="Number of invocations")
@click.option(
    "--heap-multiplier", "-m", default=None, type=float, help="Heap size as multiplier of minheap"
)
@click.option(
    "--iterations", default=None, type=int, help="DaCapo timing iterations per invocation"
)
@click.option("--profile", default="release", help="Build profile (release, fastdebug, etc.)")
@click.option(
    "--log-dir", default=None, type=click.Path(), help="Directory to store logs (default: temp dir)"
)
@click.option("--db", default=None, type=click.Path(), help="Path to SQLite database")
def run_cmd(benchmarks, plan, invocations, heap_multiplier, iterations, profile, log_dir, db):
    """Run benchmarks and record results."""
    ws = WorkspaceConfig.load()
    db_path = Path(db) if db else ws.db_path
    init_db(db_path)

    # Resolve defaults
    if benchmarks is None:
        bm_list = ws.default_benchmarks
    elif benchmarks == "all":
        bm_list = ALL_DACAPO_2006
    else:
        bm_list = [b.strip() for b in benchmarks.split(",")]

    plan = plan or ws.default_plan
    invocations = invocations or ws.default_invocations
    heap_multiplier = heap_multiplier if heap_multiplier is not None else ws.default_heap_multiplier
    iterations = iterations or ws.default_iterations

    # Get git info
    core_info = get_git_info(ws.mmtk_core)
    binding_info = get_git_info(ws.mmtk_openjdk)

    console.print("\n[bold]MMTk Performance Run[/bold]")
    console.print(f"  Core:    {core_info.get('branch', '?')} @ {core_info.get('commit', '?')[:8]}")
    console.print(
        f"  Binding: {binding_info.get('branch', '?')} @ {binding_info.get('commit', '?')[:8]}"
    )
    console.print(f"  Plan:    {plan}")
    console.print(f"  Benchmarks: {', '.join(bm_list)}")
    console.print(
        f"  Invocations: {invocations}, Heap: {heap_multiplier}x, Iterations: {iterations}"
    )

    # Detect/register testbed
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

    # Register build
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

    # Create run
    run_id = queries.create_run(
        build_id=build_id,
        testbed_id=testbed_id,
        invocations=invocations,
        heap_multiplier=heap_multiplier,
        db_path=db_path,
    )

    console.print(f"\n  Run ID:  [cyan]{run_id}[/cyan]")
    console.print(f"  Build:   [dim]{build_id}[/dim]")
    console.print()

    # Execute benchmarks
    jdk_path = ws.get_jdk_path(profile)
    if not jdk_path.exists():
        console.print(f"[red]Error: JDK not found at {jdk_path}[/red]")
        console.print(f"[dim]Build it first: cd {ws.openjdk} && make ...[/dim]")
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
        log_dir=Path(log_dir) if log_dir else None,
    )

    console.print("[bold]Running benchmarks...[/bold]")
    result = runner.run_benchmarks(run_config)

    if not result.success and not result.results:
        console.print("[red]Benchmark execution failed![/red]")
        queries.complete_run(run_id, "failed", db_path)
        raise click.Abort()

    # Store results
    db_results = results_to_db_format(result.results)
    queries.insert_results(run_id, db_results, db_path)
    queries.complete_run(run_id, "completed", db_path)

    # Update running-ng run ID
    if result.run_id:
        from ..db.schema import get_connection

        with get_connection(db_path) as conn:
            conn.execute("UPDATE run SET running_ng_id = ? WHERE id = ?", (result.run_id, run_id))

    # Print summary table
    _print_run_summary(run_id, db_path)

    console.print(f"\n[green]✓ Run {run_id} completed[/green]")
    console.print(
        f"  To set as baseline: [cyan]mmtk-dev set-baseline <name> --run-id {run_id}[/cyan]"
    )
    console.print("  To compare:         [cyan]mmtk-dev compare --baseline <name>[/cyan]")


def _print_run_summary(run_id: str, db_path: Path | None = None):
    """Print a summary table of run results."""
    grouped = queries.get_results_by_benchmark(run_id, db_path)
    if not grouped:
        console.print("[yellow]No results found.[/yellow]")
        return

    table = Table(title="Run Results")
    table.add_column("Benchmark", style="bold")
    table.add_column("Mean (ms)", justify="right")
    table.add_column("±CI", justify="right")
    table.add_column("Median (ms)", justify="right")
    table.add_column("Passed", justify="right")

    for bm, results in sorted(grouped.items()):
        times = [r["execution_time_ms"] for r in results if r["execution_time_ms"] is not None]
        stats = compute_statistics(times)

        if stats["mean"] is not None:
            mean_str = f"{stats['mean']:.1f}"
            ci_str = f"±{stats['ci']:.1f}" if stats["ci"] else ""
            median_str = f"{stats['median']:.1f}"
        else:
            mean_str = ci_str = median_str = "-"

        passed = len(times)
        total = len(results)
        passed_str = f"{passed}/{total}" if passed < total else str(passed)

        table.add_row(bm, mean_str, ci_str, median_str, passed_str)

    console.print(table)
