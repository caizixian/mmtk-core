"""Workspace configuration management (.mmtk-dev.toml)."""

import tomllib
from dataclasses import dataclass, field
from pathlib import Path

CONFIG_FILENAME = ".mmtk-dev.toml"

# DaCapo Chopin (23.11) minheap values (MB)
# TODO: These are inaccurate rough estimates. Extract real values from DaCapo's
# nominal stats mode: java -jar dacapo.jar -p <benchmark>
DACAPO_MINHEAP: dict[str, int] = {
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
