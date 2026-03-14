# Getting Started

## Prerequisites

- Python 3.11+
- [uv](https://docs.astral.sh/uv/) package manager
- A built OpenJDK with MMTk (see the main [MMTk guide](https://docs.mmtk.io))

## Installation

```bash
cd mmtk-core/tools/mmtk-dev
uv sync
```

This installs mmtk-dev and all dependencies, including `running-ng` for benchmark execution.

## First Run

### 1. Run benchmarks

Run the `fop` benchmark with 10 invocations using the GenImmix plan:

```bash
uv run mmtk-dev run -b fop -i 10 -p GenImmix
```

This will:
- Auto-detect your machine as a testbed
- Record git commits from mmtk-core and mmtk-openjdk
- Execute benchmarks via `running-ng`
- Parse results and store them in SQLite

### 2. Set a baseline

Mark the current run as your reference point:

```bash
uv run mmtk-dev set-baseline master
```

### 3. Make changes and compare

After modifying mmtk-core, rebuild and compare:

```bash
uv run mmtk-dev compare
```

This runs the same benchmarks again and shows a diff table with per-benchmark execution time improvements/regressions and a geometric mean.

## Configuration

Create a `.mmtk-dev.toml` in your workspace root for persistent configuration. See [Configuration](./configuration.md) for details.
