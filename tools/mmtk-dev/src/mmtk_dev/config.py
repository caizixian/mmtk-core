"""Workspace configuration management (.mmtk-dev.toml)."""

import os
import subprocess
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

CONFIG_FILENAME = ".mmtk-dev.toml"

# DaCapo 2006 minheap values (MB) — from ci-perf-kit running-openjdk-base.yml
DACAPO_2006_MINHEAP: dict[str, int] = {
    "antlr": 24,
    "bloat": 33,
    "eclipse": 84,
    "fop": 40,
    "xalan": 54,
    "jython": 40,
    "luindex": 22,
    "lusearch": 34,
    "pmd": 49,
    "sunflow": 54,
    "hsqldb": 127,
}

# DaCapo Chopin (23.11) minheap values — rough estimates
DACAPO_CHOPIN_MINHEAP: dict[str, int] = {
    "avrora": 32,
    "batik": 64,
    "biojava": 128,
    "cassandra": 256,
    "eclipse": 256,
    "fop": 32,
    "graphchi": 256,
    "h2": 512,
    "h2o": 256,
    "jython": 64,
    "kafka": 128,
    "luindex": 32,
    "lusearch": 64,
    "pmd": 128,
    "spring": 128,
    "sunflow": 64,
    "tomcat": 64,
    "tradebeans": 256,
    "tradesoap": 256,
    "xalan": 64,
    "zxing": 32,
}

# All supported DaCapo 2006 benchmarks
ALL_DACAPO_2006 = list(DACAPO_2006_MINHEAP.keys())

# Quick subset for development
QUICK_BENCHMARKS = ["fop"]


@dataclass
class WorkspaceConfig:
    """Configuration for an mmtk-dev workspace."""

    mmtk_core: Path = Path(".")
    mmtk_openjdk: Path = Path("../mmtk-openjdk")
    openjdk: Path = Path("../openjdk")
    testbed_id: str = ""
    testbed_name: str = ""
    api_url: str = "http://localhost:8080"
    db_path: Path | None = None
    default_plan: str = "GenImmix"
    default_benchmarks: list[str] = field(default_factory=lambda: ["fop"])
    default_invocations: int = 10
    default_heap_multiplier: float = 3.0
    default_iterations: int = 6
    dacapo_jar: Path | None = None
    dacapo_suite: str = "dacapo2006"
    probes_path: Path | None = None

    @classmethod
    def load(cls, search_dir: Path | None = None) -> "WorkspaceConfig":
        """Load config from .mmtk-dev.toml, searching upward from search_dir."""
        if search_dir is None:
            search_dir = Path.cwd()

        config_path = _find_config(search_dir)
        if config_path is None:
            return cls()

        with open(config_path, "rb") as f:
            data = tomllib.load(f)

        config = cls()
        ws = data.get("workspace", {})
        config_dir = config_path.parent

        if "mmtk_core" in ws:
            config.mmtk_core = (config_dir / ws["mmtk_core"]).resolve()
        if "mmtk_openjdk" in ws:
            config.mmtk_openjdk = (config_dir / ws["mmtk_openjdk"]).resolve()
        if "openjdk" in ws:
            config.openjdk = (config_dir / ws["openjdk"]).resolve()
        if "dacapo_jar" in ws:
            config.dacapo_jar = (config_dir / ws["dacapo_jar"]).resolve()
        if "dacapo_suite" in ws:
            config.dacapo_suite = ws["dacapo_suite"]
        if "probes_path" in ws:
            config.probes_path = (config_dir / ws["probes_path"]).resolve()
        if "db_path" in ws:
            config.db_path = Path(ws["db_path"]).expanduser()

        tb = data.get("testbed", {})
        if "id" in tb:
            config.testbed_id = tb["id"]
        if "name" in tb:
            config.testbed_name = tb["name"]

        defaults = data.get("defaults", {})
        if "plan" in defaults:
            config.default_plan = defaults["plan"]
        if "benchmarks" in defaults:
            config.default_benchmarks = defaults["benchmarks"]
        if "invocations" in defaults:
            config.default_invocations = defaults["invocations"]
        if "heap_multiplier" in defaults:
            config.default_heap_multiplier = defaults["heap_multiplier"]
        if "iterations" in defaults:
            config.default_iterations = defaults["iterations"]

        if "api" in data and "url" in data["api"]:
            config.api_url = data["api"]["url"]

        return config

    def get_jdk_path(self, debug_level: str = "release") -> Path:
        """Get the path to the built JDK."""
        return self.openjdk / f"build/linux-x86_64-server-{debug_level}/images/jdk"


def _find_config(start: Path) -> Path | None:
    """Search upward from start for .mmtk-dev.toml."""
    current = start.resolve()
    while True:
        candidate = current / CONFIG_FILENAME
        if candidate.exists():
            return candidate
        parent = current.parent
        if parent == current:
            return None
        current = parent


def get_git_info(repo_path: Path) -> dict[str, str]:
    """Get git commit and branch info for a repository."""
    info: dict[str, str] = {}
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=str(repo_path),
            capture_output=True,
            text=True,
            check=True,
        )
        info["commit"] = result.stdout.strip()

        result = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            cwd=str(repo_path),
            capture_output=True,
            text=True,
            check=True,
        )
        info["branch"] = result.stdout.strip()

        # Also try to get the remote URL
        result = subprocess.run(
            ["git", "remote", "get-url", "origin"],
            cwd=str(repo_path),
            capture_output=True,
            text=True,
            check=True,
        )
        url = result.stdout.strip()
        # Normalize github URLs to org/repo format
        if "github.com" in url:
            parts = url.rstrip(".git").split("github.com")[-1]
            parts = parts.lstrip(":").lstrip("/")
            info["repo"] = parts
        else:
            info["repo"] = url
    except (subprocess.CalledProcessError, FileNotFoundError):
        info.setdefault("commit", "unknown")
        info.setdefault("branch", "unknown")
        info.setdefault("repo", "unknown")
    return info


def detect_testbed() -> dict[str, str | int | float]:
    """Auto-detect the current machine as a testbed."""
    import platform

    info: dict[str, str | int | float] = {}
    info["name"] = platform.node()
    info["id"] = platform.node().lower().replace(".", "-")

    # CPU model
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    info["cpu_model"] = line.split(":")[1].strip()
                    break
    except FileNotFoundError:
        info["cpu_model"] = platform.processor()

    # CPU cores
    info["cpu_cores"] = os.cpu_count() or 0

    # Memory
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if line.startswith("MemTotal"):
                    kb = int(line.split()[1])
                    info["memory_gb"] = round(kb / 1024 / 1024, 1)
                    break
    except FileNotFoundError:
        pass

    return info
