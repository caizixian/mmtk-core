# Architecture

## Overview

mmtk-dev is structured as a Python package with four main components:

```
mmtk-dev/
├── src/mmtk_dev/
│   ├── cli/         # Click CLI commands
│   ├── api/         # FastAPI REST backend
│   ├── db/          # SQLite schema and queries
│   ├── runner/      # running-ng integration and log parser
│   ├── stats/       # Statistical analysis
│   └── config.py    # Workspace configuration
├── tests/           # pytest unit tests
└── docs/            # This mdbook documentation
```

## Component Details

### Runner (`runner/`)

The runner uses a **Protocol-based design** for extensibility:

- `Runner` protocol defines the `run_benchmarks()` interface
- `LocalRunner` implements local execution via `running-ng` subprocess
- Future SSH runner can implement the same protocol for remote execution

The runner generates `running-ng` YAML configs programmatically, including:
- DaCapo minheap values from ci-perf-kit
- JVM arguments for MMTk plans
- Timing iteration configuration

### Parser (`runner/parser.py`)

An independent log parser (not depending on plotty or ci-perf-kit) that:
- Parses running-ng log filenames for metadata
- Extracts DaCapo execution times (`PASSED in N msec/ms`)
- Extracts MMTk statistics blocks
- Handles `.log.gz` compressed files

### Statistics (`stats/`)

Implements the same statistical methodology as `ci-perf-kit/scripts/compare_report.py`:
- Mean, median, standard deviation
- Confidence intervals using t-distribution
- Outlier removal using z-score (threshold = 3)
- Geometric mean of ratios for overall comparison
- Change classification (faster/slower/neutral)

### Database (`db/`)

SQLite with WAL mode for concurrent access:
- Deterministic build IDs from commit + plan hash
- Per-invocation result storage (statistics computed at query time)
- Named baselines with default management

### API (`api/`)

FastAPI with CORS support, providing REST endpoints for:
- CRUD for builds, runs, results, baselines, testbeds
- Compare endpoint with statistical analysis
- Designed for the web frontend to consume

## Design Decisions

- **running-ng as subprocess**: Uses running-ng's battle-tested execution logic rather than reimplementing it
- **SQLite**: No server setup required, perfect for single-machine use; WAL mode supports concurrent reads
- **Per-invocation storage**: Raw data stored, statistics computed on the fly — enables different analysis after the fact
- **Runner protocol**: Abstracts execution location for future SSH support without changing the data flow
