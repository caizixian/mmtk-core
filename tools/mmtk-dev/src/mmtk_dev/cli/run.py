"""The 'run' command: execute benchmarks and store results."""

from pathlib import Path

import click
from rich.console import Console
from rich.table import Table

from ..config import WorkspaceConfig
from ..db import queries
from ..db.schema import init_db
from ..stats.analysis import compute_statistics
from ._orchestrate import execute_run, register_environment, resolve_benchmarks, resolve_defaults

console = Console()


@click.command("run")
@click.option(
    "--benchmarks",
    "-b",
    default=None,
    help="Comma-separated benchmark names, or 'all' for all DaCapo Chopin",
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
@click.option("--debug", is_flag=True, help="Print the generated running-ng config and commands")
def run_cmd(benchmarks, plan, invocations, heap_multiplier, iterations, profile, log_dir, db, debug):
    """Run benchmarks and record results."""
    ws = WorkspaceConfig.load()
    db_path = Path(db) if db else ws.db_path
    init_db(db_path)

    bm_list = resolve_benchmarks(benchmarks, ws)
    plan, invocations, heap_multiplier, iterations = resolve_defaults(
        ws, plan=plan, invocations=invocations,
        heap_multiplier=heap_multiplier, iterations=iterations,
    )

    build_id, testbed_id, core_info, binding_info = register_environment(
        ws, db_path, plan=plan, profile=profile,
    )

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

    # Check JDK exists
    jdk_path = ws.get_jdk_path(profile)
    if not jdk_path.exists():
        console.print(f"[red]Error: JDK not found at {jdk_path}[/red]")
        console.print(f"[dim]Build it first: cd {ws.openjdk} && make ...[/dim]")
        raise click.Abort()

    # Debug: show generated config
    if debug:
        import yaml

        from ..runner.runner import LocalRunner, RunConfig

        runner = LocalRunner(ws)
        run_config = RunConfig(
            benchmarks=bm_list, plan=plan, jdk_path=jdk_path,
            invocations=invocations, heap_multiplier=heap_multiplier,
            iterations=iterations, suite="dacapochopin",
            dacapo_jar=ws.dacapo_jar, probes_path=ws.probes_path,
        )
        running_config = runner.generate_config(run_config)
        console.print("\n[bold yellow]── Generated running-ng config ──[/bold yellow]")
        console.print(yaml.dump(running_config, default_flow_style=False))
        console.print("[bold yellow]── End config ──[/bold yellow]\n")

    orch = execute_run(
        ws, db_path,
        build_id=build_id, testbed_id=testbed_id,
        benchmarks=bm_list, plan=plan, profile=profile,
        invocations=invocations, heap_multiplier=heap_multiplier,
        iterations=iterations, log_dir=Path(log_dir) if log_dir else None,
    )

    if not orch.success:
        console.print("[red]Benchmark execution failed![/red]")
        raise click.Abort()

    console.print(f"\n  Run ID:  [cyan]{orch.run_id}[/cyan]")
    console.print(f"  Build:   [dim]{orch.build_id}[/dim]")
    console.print()

    _print_run_summary(orch.run_id, db_path)

    console.print(f"\n[green]✓ Run {orch.run_id} completed[/green]")
    console.print(
        f"  To set as baseline: [cyan]mmtk-dev set-baseline <name> --run-id {orch.run_id}[/cyan]"
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
