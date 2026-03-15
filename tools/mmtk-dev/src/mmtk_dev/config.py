"""Workspace configuration management (.mmtk-dev.toml)."""

import tomllib
from dataclasses import dataclass, field
from pathlib import Path

CONFIG_FILENAME = ".mmtk-dev.toml"

# DaCapo Chopin (23.11) minheap values (MB)
DACAPO_MINHEAP: dict[str, int] = {
    "avrora": 5,
    "batik": 175,
    "biojava": 93,
    "cassandra": 174,
    "eclipse": 135,
    "fop": 13,
    "graphchi": 175,
    "h2": 681,
    "h2o": 72,
    "jme": 29,
    "jython": 31,
    "kafka": 208,
    "luindex": 31,
    "lusearch": 21,
    "pmd": 269,
    "spring": 70,
    "sunflow": 31,
    "tomcat": 24,
    "tradebeans": 141,
    "tradesoap": 115,
    "xalan": 17,
    "zxing": 127,
}

# All supported DaCapo Chopin benchmarks
ALL_DACAPO = list(DACAPO_MINHEAP.keys())

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
    gc_threads: int | None = None
    app_threads: int | None = None
    dacapo_jar: Path | None = None
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
        if "gc_threads" in defaults:
            config.gc_threads = defaults["gc_threads"]
        if "app_threads" in defaults:
            config.app_threads = defaults["app_threads"]

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
