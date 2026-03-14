"""The 'server' command: start the API backend and serve the web frontend."""

import os
import subprocess
from pathlib import Path

import click
from rich.console import Console

console = Console()

_FRONTEND_DIR = Path(__file__).parent.parent.parent.parent / "frontend"


def _build_frontend() -> bool:
    """Build the React frontend using npm run build."""
    if not _FRONTEND_DIR.exists():
        console.print("[yellow]⚠ Frontend directory not found, skipping build[/yellow]")
        return False

    # Check if node_modules exist, install if not
    node_modules = _FRONTEND_DIR / "node_modules"
    if not node_modules.exists():
        console.print("[dim]Installing frontend dependencies...[/dim]")
        result = subprocess.run(
            ["npm", "install"],
            cwd=str(_FRONTEND_DIR),
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            console.print(f"[red]npm install failed:[/red]\n{result.stderr}")
            return False

    console.print("[dim]Building frontend...[/dim]")
    result = subprocess.run(
        ["npm", "run", "build"],
        cwd=str(_FRONTEND_DIR),
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        console.print(f"[red]Frontend build failed:[/red]\n{result.stderr}")
        return False

    console.print("[green]✓ Frontend built successfully[/green]")
    return True


@click.command("server")
@click.option("--port", default=8080, type=int, help="Port to listen on")
@click.option("--host", default="127.0.0.1", help="Host to bind to")
@click.option("--db", default=None, type=click.Path(), help="Path to database")
@click.option("--skip-build", is_flag=True, help="Skip frontend build")
def server_cmd(port: int, host: str, db: str | None, skip_build: bool) -> None:
    """Start the mmtk-dev API server and web dashboard.

    Builds the React frontend (if needed) and starts the FastAPI server.
    """
    import uvicorn

    from ..config import WorkspaceConfig
    from ..db.schema import init_db

    ws = WorkspaceConfig.load()
    db_path = Path(db) if db else ws.db_path
    init_db(db_path)

    # Set db_path as environment variable for the API to pick up
    if db_path:
        os.environ["MMTK_DEV_DB_PATH"] = str(db_path)

    # Build frontend
    if not skip_build:
        _build_frontend()

    console.print("\n[bold]MMTk Dev Server[/bold]")
    console.print(f"  API:       http://{host}:{port}/api")
    console.print(f"  Dashboard: http://{host}:{port}")
    console.print(f"  Database:  {db_path or 'default'}")
    console.print()

    uvicorn.run(
        "mmtk_dev.api.api:app",
        host=host,
        port=port,
        reload=False,
    )
