"""Main CLI entry point for mmtk-dev."""

import click

from .baseline import set_baseline_cmd
from .ci import ci_cmd
from .compare import compare_cmd
from .run import run_cmd
from .server import server_cmd


@click.group()
@click.version_option(version="0.1.0")
def main():
    """MMTk performance tracking and regression detection tool."""
    pass


main.add_command(run_cmd, "run")
main.add_command(compare_cmd, "compare")
main.add_command(set_baseline_cmd, "set-baseline")
main.add_command(ci_cmd, "ci")
main.add_command(server_cmd, "server")


if __name__ == "__main__":
    main()
