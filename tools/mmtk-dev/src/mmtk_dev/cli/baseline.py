"""The 'set-baseline' command: mark a run as a named baseline."""

from pathlib import Path

import click
from rich.console import Console

from ..config import WorkspaceConfig
from ..db import queries
from ..db.schema import init_db

console = Console()


@click.command("set-baseline")
@click.argument("name")
@click.option("--run-id", default=None, help="Run ID to use (default: latest run)")
@click.option(
    "--no-default", is_flag=True, help="Don't set this baseline as the default for comparisons"
)
@click.option("--description", "-d", default=None, help="Description of this baseline")
@click.option("--db", default=None, type=click.Path(), help="Path to SQLite database")
def set_baseline_cmd(name, run_id, no_default, description, db):
    """Set a run as a named baseline for future comparisons.

    NAME is a short identifier for the baseline (e.g. 'master', 'before-refactor').
    """
    ws = WorkspaceConfig.load()
    db_path = Path(db) if db else ws.db_path
    init_db(db_path)

    # Find the run
    if run_id is None:
        run = queries.get_latest_run(db_path)
        if run is None:
            console.print("[red]No runs found. Run benchmarks first with 'mmtk-dev run'.[/red]")
            raise click.Abort()
        run_id = run["id"]
    else:
        run = queries.get_run(run_id, db_path)
        if run is None:
            console.print(f"[red]Run '{run_id}' not found.[/red]")
            raise click.Abort()

    # Get build info for display
    build = queries.get_build(run["build_id"], db_path)

    # Set the baseline
    is_default = not no_default
    queries.set_baseline(
        name, run_id, is_default=is_default, description=description, db_path=db_path
    )

    console.print(f"\n[green]✓ Baseline '{name}' set[/green]")
    console.print(f"  Run:     {run_id}")
    if build:
        console.print(f"  Commit:  {build['core_commit'][:8]}")
        console.print(f"  Plan:    {build['gc_plan']}")
    console.print(f"  Default: {'yes' if is_default else 'no'}")

    if is_default:
        console.print("\n  Compare against it: [cyan]mmtk-dev compare[/cyan]")
    else:
        console.print(f"\n  Compare against it: [cyan]mmtk-dev compare --baseline {name}[/cyan]")
