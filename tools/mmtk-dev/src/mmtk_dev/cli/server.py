"""The 'server' command: start the API backend and serve the web frontend."""

import click
from rich.console import Console

console = Console()


@click.command("server")
@click.option("--port", default=8080, type=int, help="Port to listen on")
@click.option("--host", default="127.0.0.1", help="Host to bind to")
@click.option("--db", default=None, type=click.Path(), help="Path to database")
def server_cmd(port, host, db):
    """Start the mmtk-dev API server and web dashboard."""
    from pathlib import Path

    import uvicorn

    from ..config import WorkspaceConfig
    from ..db.schema import init_db

    ws = WorkspaceConfig.load()
    db_path = Path(db) if db else ws.db_path
    init_db(db_path)

    # Set db_path as environment variable for the API to pick up
    import os

    if db_path:
        os.environ["MMTK_DEV_DB_PATH"] = str(db_path)

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
