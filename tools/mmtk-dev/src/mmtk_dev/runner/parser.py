"""Log parser for running-ng output files.

Parses .log.gz files produced by running-ng's runbms command, extracting
benchmark execution times and MMTk statistics.

Log filename format: {benchmark}.{hfac_int}.{heap_mb}.{config}.{suite}.log.gz
"""

import gzip
import re
import os
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class BenchmarkResult:
    """A single benchmark's results from a log file."""
    benchmark: str
    suite: str
    config: str
    heap_size_mb: int
    heap_factor: float  # hfac_int / 1000
    execution_times: list[float] = field(default_factory=list)
    mmtk_stats: list[dict[str, float]] = field(default_factory=list)
    log_file: str = ""

    @property
    def n_passed(self) -> int:
        return len(self.execution_times)


# Regex for DaCapo "PASSED in N msec"
_PASSED_RE = re.compile(r"PASSED in (\d+) msec")

# Regex for DaCapo Chopin "PASSED in N ms"
_PASSED_MS_RE = re.compile(r"PASSED in (\d+) ms")

# Regex for the MMTk statistics header
_MMTK_STATS_HEADER = "MMTk Statistics Totals"

# Regex for log filenames
# e.g. "fop.3000.120.jdk.tph.ms.plan.dacapo2006.log.gz"
# benchmark.hfac_int.heap_mb.config_parts.suite.log.gz
_LOG_FILENAME_RE = re.compile(
    r"^(?P<benchmark>[^.]+)"
    r"\.(?P<hfac>\d+)"
    r"\.(?P<heap>\d+)"
    r"\.(?P<config>.+)"
    r"\.(?P<suite>[^.]+)"
    r"\.log(?:\.gz)?$"
)


def parse_log_filename(filename: str) -> dict | None:
    """Parse a running-ng log filename into components.

    Returns None if the filename doesn't match the expected pattern.
    """
    m = _LOG_FILENAME_RE.match(filename)
    if not m:
        return None
    return {
        "benchmark": m.group("benchmark"),
        "hfac_int": int(m.group("hfac")),
        "heap_size_mb": int(m.group("heap")),
        "config": m.group("config"),
        "suite": m.group("suite"),
    }


def parse_log_content(content: str) -> tuple[list[float], list[dict[str, float]]]:
    """Parse log file content for execution times and MMTk statistics.

    Returns (execution_times, mmtk_stats_per_invocation).
    """
    lines = content.splitlines()
    execution_times: list[float] = []
    mmtk_stats: list[dict[str, float]] = []

    for i, line in enumerate(lines):
        # Check for "PASSED in N msec" or "PASSED in N ms"
        m = _PASSED_RE.search(line) or _PASSED_MS_RE.search(line)
        if m:
            execution_times.append(float(m.group(1)))

        # Check for MMTk statistics block
        if _MMTK_STATS_HEADER in line:
            # Next line = keys, line after = values
            if i + 2 < len(lines):
                keys_line = lines[i + 1].strip()
                values_line = lines[i + 2].strip()
                keys = keys_line.split()
                values = values_line.split()
                if len(keys) == len(values):
                    try:
                        stats = {k: float(v) for k, v in zip(keys, values)}
                        mmtk_stats.append(stats)
                    except ValueError:
                        pass  # Skip malformed stats

    return execution_times, mmtk_stats


def parse_log_file(path: Path) -> BenchmarkResult | None:
    """Parse a single .log.gz or .log file.

    Returns a BenchmarkResult, or None if the file can't be parsed.
    """
    filename = path.name
    parsed = parse_log_filename(filename)
    if parsed is None:
        return None

    # Read file content
    try:
        if filename.endswith(".gz"):
            with gzip.open(str(path), "rt", errors="replace") as f:
                content = f.read()
        else:
            with open(path, "r", errors="replace") as f:
                content = f.read()
    except (OSError, gzip.BadGzipFile):
        return None

    execution_times, mmtk_stats = parse_log_content(content)

    return BenchmarkResult(
        benchmark=parsed["benchmark"],
        suite=parsed["suite"],
        config=parsed["config"],
        heap_size_mb=parsed["heap_size_mb"],
        heap_factor=parsed["hfac_int"] / 1000.0,
        execution_times=execution_times,
        mmtk_stats=mmtk_stats,
        log_file=filename,
    )


def parse_run_directory(run_dir: Path) -> list[BenchmarkResult]:
    """Parse all log files in a running-ng run directory.

    Returns a list of BenchmarkResult objects.
    """
    results: list[BenchmarkResult] = []

    if not run_dir.is_dir():
        return results

    for entry in sorted(run_dir.iterdir()):
        if entry.name.endswith(".log.gz") or entry.name.endswith(".log"):
            result = parse_log_file(entry)
            if result is not None:
                results.append(result)

    return results


def results_to_db_format(results: list[BenchmarkResult]) -> list[dict]:
    """Convert parsed results to the format expected by db.queries.insert_results."""
    db_results: list[dict] = []

    for r in results:
        for i, time_ms in enumerate(r.execution_times):
            db_results.append({
                "benchmark": r.benchmark,
                "suite": r.suite,
                "heap_size_mb": r.heap_size_mb,
                "heap_factor": r.heap_factor,
                "invocation": i,
                "execution_time_ms": time_ms,
                "status": "pass",
            })

        # If we expected more invocations than we got, some failed
        # But we don't know the exact count here, so just record what passed

    return db_results
