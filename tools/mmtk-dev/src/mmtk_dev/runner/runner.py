"""Running-ng integration: config generation and benchmark execution.

Uses a Runner protocol so execution can be extended to remote (SSH) later.
"""

import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

import yaml

from ..config import (
    DACAPO_2006_MINHEAP,
    DACAPO_CHOPIN_MINHEAP,
    WorkspaceConfig,
)
from .parser import BenchmarkResult, parse_run_directory


@dataclass
class RunConfig:
    """Configuration for a benchmark run."""

    benchmarks: list[str]
    plan: str
    jdk_path: Path
    invocations: int = 10
    heap_multiplier: float = 3.0
    iterations: int = 6
    suite: str = "dacapo2006"
    dacapo_jar: Path | None = None
    log_dir: Path | None = None  # If None, uses temp dir
    probes_path: Path | None = None  # Path to probes repo for MMTk stats


@dataclass
class RunResult:
    """Result of a benchmark execution."""

    run_id: str  # running-ng run ID
    log_dir: Path
    results: list[BenchmarkResult]
    success: bool


class Runner(Protocol):
    """Protocol for benchmark runners. Extensible to SSH later."""

    def run_benchmarks(self, config: RunConfig) -> RunResult: ...


# DaCapo Chopin suite names (all variants)
_CHOPIN_SUITES = {"dacapochopin", "dacapochopin-29a657f"}

# DaCapo callback class for each suite family
_SUITE_CALLBACKS = {
    "dacapochopin": "probe.DacapoChopinCallback",
    "dacapochopin-29a657f": "probe.DacapoChopinCallback",
    "dacapobach": "probe.DacapoBachCallback",
    "dacapobach-mr1": "probe.DacapoBachCallback",
    "dacapo2006": "probe.Dacapo2006Callback",
}


class LocalRunner:
    """Runs benchmarks on the local machine via running-ng subprocess."""

    def __init__(self, workspace: WorkspaceConfig):
        self.workspace = workspace

    def generate_config(self, config: RunConfig) -> dict:
        """Generate a running-ng YAML config dict."""
        # Pick minheap values based on suite
        all_minheap = DACAPO_2006_MINHEAP if config.suite == "dacapo2006" else DACAPO_CHOPIN_MINHEAP

        # Filter minheap to only requested benchmarks
        minheap_values = {bm: all_minheap.get(bm, 64) for bm in config.benchmarks}

        # Build the config string parts
        config_parts = ["jdk", "common_mmtk"]

        # Add DaCapo Chopin JDK21 compatibility modifiers
        if config.suite in _CHOPIN_SUITES:
            config_parts.append("dacapochopin_jdk21")

        running_config: dict = {
            "includes": ["$RUNNING_NG_PACKAGE_DATA/base/runbms.yml"],
            "benchmarks": {
                config.suite: config.benchmarks,
            },
            "overrides": {
                f"suites.{config.suite}.timing_iteration": config.iterations,
                f"suites.{config.suite}.minheap": "mmtk_dev",
                f"suites.{config.suite}.minheap_values.mmtk_dev": minheap_values,
                "remote_host": None,
            },
            "runtimes": {
                "jdk": {
                    "type": "OpenJDK",
                    "release": 21,
                    "home": str(config.jdk_path),
                },
            },
            "modifiers": {
                "tph": {
                    "type": "JVMArg",
                    "val": "-XX:+UseThirdPartyHeap",
                },
                "ms": {
                    "type": "JVMArg",
                    "val": "-XX:MetaspaceSize=500M -XX:+DisableExplicitGC",
                },
                "plan": {
                    "type": "JVMArg",
                    "val": f"-XX:ThirdPartyHeapOptions=plan={config.plan}",
                },
                "common_mmtk": {
                    "type": "ModifierSet",
                    "val": "tph|ms|plan",
                },
            },
            "configs": ["|".join(config_parts)],
        }

        # Add dacapo jar path override if specified
        if config.dacapo_jar:
            running_config["overrides"][f"suites.{config.suite}.path"] = str(config.dacapo_jar)

        # Add probes integration for MMTk statistics collection
        if config.probes_path:
            probes_out = config.probes_path / "out"
            probes_jar = probes_out / "probes.jar"

            running_config["modifiers"]["probes_cp"] = {
                "type": "JVMClasspath",
                "val": f"{probes_out} {probes_jar}",
            }
            running_config["modifiers"]["probes"] = {
                "type": "JVMArg",
                "val": f"-Djava.library.path={probes_out} -Dprobes=RustMMTk",
            }

            # Add probes to the config string
            config_parts.extend(["probes_cp", "probes"])
            running_config["configs"] = ["|".join(config_parts)]

            # Set the DaCapo callback for probes
            callback = _SUITE_CALLBACKS.get(config.suite)
            if callback:
                running_config["overrides"][f"suites.{config.suite}.callback"] = callback

        return running_config

    def run_benchmarks(self, config: RunConfig) -> RunResult:
        """Run benchmarks using running-ng."""
        # Generate config
        running_config = self.generate_config(config)

        # Determine log directory
        log_dir = config.log_dir or Path(tempfile.mkdtemp(prefix="mmtk-dev-"))

        log_dir.mkdir(parents=True, exist_ok=True)

        # Write config to temp file
        config_file = log_dir / "mmtk-dev-config.yml"
        with open(config_file, "w") as f:
            yaml.dump(running_config, f, default_flow_style=False)

        # Build the running runbms command
        cmd = [
            "running",
            "runbms",
            str(log_dir),
            str(config_file),
            "-i",
            str(config.invocations),
        ]
        if config.heap_multiplier > 0:
            cmd.extend(["-s", str(config.heap_multiplier)])

        # Run benchmarks
        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                cwd=str(log_dir),
            )

            # Extract run ID from output
            # Output format: "Run id: hostname-2024-03-14-Thu-123456"
            run_id = ""
            for line in result.stdout.splitlines():
                if "Run id:" in line or "run id:" in line.lower():
                    run_id = line.split(":")[-1].strip()
                    break

            if not run_id:
                # Fallback: look for directories in log_dir
                for d in sorted(log_dir.iterdir()):
                    if d.is_dir() and d.name != "__pycache__":
                        run_id = d.name
                        break

            # Parse results
            results_dir = log_dir / run_id if run_id else log_dir
            results = parse_run_directory(results_dir)

            return RunResult(
                run_id=run_id,
                log_dir=results_dir,
                results=results,
                success=result.returncode == 0,
            )

        except FileNotFoundError:
            return RunResult(
                run_id="",
                log_dir=log_dir,
                results=[],
                success=False,
            )
