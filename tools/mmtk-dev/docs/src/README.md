# mmtk-dev

**MMTk performance tracking and regression detection tool.**

mmtk-dev streamlines the performance analysis workflow for MMTk researchers. It provides:

- **CLI Tool** — run benchmarks, set baselines, and compare results with one command
- **Database** — SQLite-backed storage for all benchmark results with full provenance tracking
- **Statistical Analysis** — confidence intervals, outlier removal, and geometric mean comparisons
- **CI Integration** — regression detection with markdown reports for GitHub Actions
- **API Server** — REST API for programmatic access and web dashboard integration

## Quick Start

```bash
# Install
cd mmtk-core/tools/mmtk-dev
uv sync

# Run benchmarks (uses running-ng under the hood)
uv run mmtk-dev run -b fop -i 10 -p GenImmix

# Set current run as baseline
uv run mmtk-dev set-baseline master

# Compare after making changes
uv run mmtk-dev compare
```

## Why mmtk-dev?

The existing performance workflow requires switching between multiple tools:
1. Writing `running-ng` YAML configs manually
2. Parsing logs with ad-hoc scripts from `ci-perf-kit`
3. Using `plotty` (Python 2.7) for visualization
4. Copying data between machines

mmtk-dev unifies this into a single tool with persistent storage, reproducible comparisons, and both human and machine-friendly output.
