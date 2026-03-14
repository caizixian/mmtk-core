---
name: performance-benchmarking
description: How to use mmtk-dev to benchmark, measure, and compare performance when making optimizations to mmtk-core, including build, run, baseline, compare, and statistical analysis workflows.
---

# Performance Benchmarking with mmtk-dev

This skill covers the end-to-end workflow for measuring and validating performance when making optimizations to mmtk-core. The primary tool is **mmtk-dev** (`mmtk-core/tools/mmtk-dev`), which wraps `running-ng` for benchmark execution and provides statistical comparison, baseline management, and result storage.

## Prerequisites

Before benchmarking, ensure the following are set up:

1. **Python 3.11+** with [uv](https://docs.astral.sh/uv/)
2. **DaCapo Chopin** benchmark jar (`dacapo-23.11-MR2-chopin.jar`) in the workspace root
3. **Probes** built from [anupli/probes](https://github.com/anupli/probes) — enables GC stats collection (GC count, STW time, etc.)
4. **`.mmtk-dev.toml`** at the workspace root (parent of `mmtk-core/`, `mmtk-openjdk/`, `openjdk/`)

### Workspace Configuration (`.mmtk-dev.toml`)

Test with simple benchmarks first before running with the full suite.

```toml
[workspace]
mmtk_core = "./mmtk-core"
mmtk_openjdk = "./mmtk-openjdk"
openjdk = "./openjdk"
dacapo_jar = "./dacapo-23.11-MR2-chopin.jar"
probes_path = "./probes"
db_path = "~/.mmtk-dev/mmtk-dev.db"

[defaults]
plan = "GenImmix"
benchmarks = ["fop"]
invocations = 10
heap_multiplier = 3.0
iterations = 6
```

- **`probes_path`**: When set, mmtk-dev auto-adds probes classpath, JVM args (`-Dprobes=RustMMTk`), and the DaCapo callback for MMTk statistics.
- **`heap_multiplier`**: Heap size = multiplier × minheap. Use 2–3× for stress-testing GC. The minheap values are defined in `mmtk-core/tools/mmtk-dev/src/mmtk_dev/config.py` (`DACAPO_MINHEAP` dict).
- **`iterations`**: DaCapo timing iterations per invocation. More iterations = more warmup.
- **`invocations`**: Number of JVM invocations. Use ≥10 for meaningful CI, ≥20 for CI mode.

## The Performance Optimization Workflow

```
 1. Build baseline   →   2. Run benchmarks   →   3. Set baseline
                                                        ↓
 6. Interpret results  ←  5. Compare          ←   4. Make changes & rebuild
```

### Step 1: Build the Baseline

Build OpenJDK with the unmodified mmtk-core (release profile for benchmarking):

```fish
cd openjdk
sh configure --disable-warnings-as-errors --with-debug-level=release
make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images
```

### Step 2: Run Benchmarks

All `mmtk-dev` commands must be run from the **workspace root** (the directory containing `.mmtk-dev.toml`).

```fish
# Run from workspace root. Use --project to point at mmtk-dev.
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop,lusearch,xalan -i 10
```

**Key options:**
| Option | Description | Example |
|--------|-------------|---------|
| `-b, --benchmarks` | Comma-separated names, or `all` | `-b fop,lusearch` |
| `-p, --plan` | GC plan | `-p GenImmix` |
| `-i, --invocations` | Number of JVM invocations | `-i 10` |
| `-m, --heap-multiplier` | Heap size = N × minheap | `-m 3.0` |
| `--iterations` | DaCapo timing iterations per invocation | `--iterations 6` |
| `--profile` | Build profile (`release`, `fastdebug`) | `--profile release` |
| `--debug` | Print the generated running-ng YAML config | `--debug` |

**Quick benchmarks for development iteration:**
```fish
# Single fast benchmark, 2 invocations — good for sanity checks
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop -i 2 --iterations 1
```

**Thorough benchmarks for validation:**
```fish
# Multiple benchmarks, 10+ invocations — needed for statistical confidence
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop,lusearch,xalan -i 10
```

### Step 3: Set Baseline

Mark the current run as the reference point:

```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev set-baseline before-opt
```

Options:
- `--run-id <id>` — use a specific run (default: latest)
- `--no-default` — don't set as the default baseline for `compare`
- `-d <text>` — add a description

### Step 4: Make Changes and Rebuild

Edit mmtk-core, then rebuild:

```fish
cd openjdk
make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images
```

> [!IMPORTANT]
> **Commit your changes before running benchmarks.** `mmtk-dev` captures the git commit hashes of `mmtk-core` and `mmtk-openjdk` for each run. If you run benchmarks with uncommitted changes, the recorded commit hash won't reflect the actual code that was benchmarked, making results harder to trace back. Always `git commit` before proceeding to Step 5.

### Step 5: Compare

Run the same benchmarks and compare against the baseline:

```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare
```

This will:
1. Run the same benchmarks as the baseline
2. Show a per-benchmark comparison table with mean, CI, diff%, and status
3. Output a geometric mean summary

You can also compare an **existing run** without re-running benchmarks:
```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare --run-id <run-id>
```

**Key options:**
| Option | Description | Default |
|--------|-------------|---------|
| `-b, --baseline` | Baseline name | Default baseline |
| `--benchmarks` | Override benchmarks | Same as baseline |
| `--run-id` | Compare existing run (skip running) | |
| `--threshold` | Significance threshold | `0.02` (2%) |

### Step 6: Interpret Results

The output table looks like:

```
┃ Benchmark ┃  Baseline (ms) ┃   Current (ms) ┃   Diff ┃   Status    ┃
│ fop       │ 3262.5 ±1429.4 │ 3100.0 ±500.0  │ -4.98% │ ✅ faster   │
│ lusearch  │  850.0 ±12.3   │  870.0 ±15.1   │ +2.35% │ ❌ slower   │
│ xalan     │  450.0 ±8.5    │  445.0 ±9.2    │ -1.11% │ ➡️ neutral  │

Geometric mean: -1.25% (improvement)
```

**Classification rules** (from `stats/analysis.py`):
- **faster** (✅): diff ≤ −threshold (default −2%)
- **slower** (❌): diff ≥ +threshold (default +2%)
- **neutral** (➡️): within ±threshold

**Statistics computed per benchmark:**
- Mean, median, standard deviation
- 95% confidence interval (t-distribution)
- Outlier removal via z-score (threshold = 3)
- Geometric mean of ratios across all benchmarks

**Signs of trustworthy results:**
- CI (±) is small relative to the mean (< 5% of mean is good)
- Number of invocations ≥ 10
- No outliers removed (or very few)

**Signs of noisy results:**
- Large CI relative to mean → increase invocations or reduce system noise
- Many outliers removed → check for system interference

## Choosing Benchmarks

Test with small or optimization-pertinent benchmarks first. Only run the full suite once you are confident the change is correct and beneficial.

### DaCapo Chopin Benchmark Reference

| Benchmark | Minheap (MB) | Threading | Description | GC-Relevant Characteristics |
|-----------|:-----------:|-----------|-------------|---------------------------|
| `fop` | 13 | Sequential | XSL-FO → PDF formatter | Very small heap, fast to run. Good smoke test. Low allocation pressure. |
| `avrora` | 5 | Multi (fine-grained) | AVR microcontroller simulator | Tiny heap, many fine-grained thread interactions. Tests synchronization. |
| `lusearch` | 21 | Multi (partitioned) | Lucene text search over corpus | Allocation-heavy, high GC stress. Very sensitive to allocation path and nursery performance. |
| `xalan` | 17 | Multi | XSLT processor (XML → HTML) | Moderate allocation, multi-threaded. Good general-purpose GC benchmark. |
| `luindex` | 31 | Sequential | Lucene text indexing | Sequential indexer. Tests allocation path without threading effects. |
| `sunflow` | 31 | Multi (parallel) | Ray-tracing renderer | Embarrassingly parallel. Tests GC scaling with many threads. |
| `tomcat` | 24 | Multi | Apache Tomcat web server | Realistic server workload. Request/response allocation patterns. |
| `jme` | 29 | Multi | 3D game engine (jMonkeyEngine) | Short-lived objects in a game loop. GPU rendering. Very little CPU workload. |
| `jython` | 31 | Sequential | Python interpreter on JVM | Interpreter overhead. Exercises object allocation and finalization. |
| `h2o` | 72 | Multi | H2O machine-learning framework | Medium heap, analytical workload. |
| `spring` | 70 | Multi | Spring Boot web application | Latency-sensitive server workload. Tests GC pause impact. |
| `biojava` | 93 | Multi | Bioinformatics sequence analysis | Medium heap, scientific computation. A big array (genomic sequence) pointing to few hot objects (nucleotides).|
| `tradesoap` | 115 | Multi | DayTrader via SOAP/Web Services | Enterprise workload with XML serialization. |
| `zxing` | 127 | Multi | Barcode/QR code processing | Image processing with moderate allocation. PNG file are read to memory. |
| `eclipse` | 135 | Multi | Eclipse IDE JDT compiler | Complex object graphs. Tests tracing and marking performance. |
| `tradebeans` | 141 | Multi | DayTrader via EJB/JavaBeans | Enterprise workload with large object state. |
| `batik` | 175 | Multi (workers) | SVG image rendering | Image transcoding + rendering workers. Tests allocation of large arrays. |
| `cassandra` | 174 | Multi | Apache Cassandra + YCSB | Large heap, database workload. Stress-tests old-gen collection. |
| `graphchi` | 175 | Multi | Disk-based graph computation | Large heap, graph processing. Tests large object management. |
| `kafka` | 208 | Multi | Apache Kafka message broker | Large heap, streaming. Network buffer allocation patterns. |
| `pmd` | 269 | Multi | Static analysis tool | Large AST allocation, deep object graphs. Tests tracing throughput. |
| `h2` | 681 | Multi (clients) | In-memory database (JDBCbench) | Largest heap. Many concurrent clients. Intensive GC stress at scale. |

### Tiered Benchmarking Strategy

**Phase 1 — Quick iteration** (during development):
Pick 1–3 small benchmarks that are relevant to the specific optimization. Run with few invocations for fast feedback.

```fish
# Example: optimizing allocation path → lusearch is allocation-heavy
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b lusearch -i 3 --iterations 1
```

**Phase 2 — Validation** (once results look promising):
Add a wider range of benchmarks with ≥10 invocations for statistical confidence.

```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop,lusearch,xalan,sunflow,h2 -i 10
```

**Phase 3 — Full suite** (before finalizing):
Run all benchmarks with ≥20 invocations to confirm no regressions anywhere.

```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b all -i 20
```

### Choosing Benchmarks by Optimization Type

| Optimization area | Recommended benchmarks | Why |
|-------------------|----------------------|-----|
| Allocation fast-path | `lusearch`, `xalan`, `sunflow` | High allocation rates, sensitive to alloc overhead |
| Nursery / minor GC | `lusearch`, `jme`, `fop` | Many short-lived objects, frequent nursery collections |
| Tracing / marking | `eclipse`, `pmd`, `h2` | Deep/complex object graphs, large live sets |
| Copying / defrag | `lusearch`, `xalan`, `h2` | Significant survivor volumes |
| STW pauses | `spring`, `tomcat`, `cassandra` | Latency-sensitive server workloads |
| Concurrent / parallel GC | `sunflow`, `h2`, `kafka` | Multi-threaded, test GC scaling |
| Large heap / old-gen | `h2`, `cassandra`, `pmd`, `kafka` | Large live heaps, exercise old-gen collection |
| Sweep performance | `lusearch`, `xalan`, `fop` | High churn → many dead objects to sweep |

## GC Plans

Available plans (set via `-p` or `--plan`):

| Plan | Description |
|------|-------------|
| `GenImmix` | Generational Immix (default, most commonly tested) |
| `Immix` | Non-generational Immix |
| `SemiSpace` | Semi-space copying collector |
| `MarkSweep` | Mark-sweep collector |
| `MarkCompact` | Mark-compact collector |
| `PageProtect` | For debugging: protects pages |
| `NoGC` | No garbage collection (useful for allocation-only benchmarks) |

Set the plan via the CLI or `.mmtk-dev.toml`:
```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop -p Immix -i 10
```

To benchmark the **same change across multiple plans**, use the `ci` command:
```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev ci --plans GenImmix,Immix -b fop,lusearch -i 10
```

## Build Profiles

| Profile | Optimized? | Debug Info? | Assertions? | Cargo Profile | Use for |
|---------|:----------:|:-----------:|:-----------:|:-------------:|---------|
| `release` | ✔ | ✘ | ✘ | release | Benchmarking |
| `fastdebug` | ✔ | ✔ | ✔ | debug | Development + debugging |
| `slowdebug` | ✘ | ✔ | ✔ | debug | Detailed debugging |
| `optimized` | ✔ | ✘ | ✘ | debug | Debugging optimized code |

> **Always use `release` for performance measurements.** Other profiles have different optimization levels and assertions that skew results.

## Benchmarking Best Practices

### Reducing Noise

1. **System isolation**: Close other applications, disable CPU frequency scaling if possible
2. **Sufficient invocations**: Use ≥10 for development, ≥20 for publishable results
3. **Consistent heap size**: Always use the same `--heap-multiplier` between baseline and comparison
4. **Same testbed**: Compare results from the same machine only
5. **Same plan and profile**: Don't compare GenImmix release against Immix fastdebug

### Iterative Optimization Pattern

```fish
# 1. Establish baseline on the unmodified code
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop,lusearch,xalan -i 10
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev set-baseline before-opt

# 2. Make your optimization changes in mmtk-core, then rebuild
cd openjdk
make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images

# 3. Compare
cd ..  # back to workspace root
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare

# 4. If results look promising, do a thorough run
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop,lusearch,xalan,h2,sunflow -i 20
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare --run-id <new-run-id>
```

### Quick A/B Comparison (No Baseline Setup)

If you just want to compare two existing runs:
```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare --run-id <run-id> --baseline <baseline-name>
```

## Web Dashboard

For visual analysis and trend tracking:

```fish
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev server
# Opens at http://127.0.0.1:8080
```

The dashboard provides:
- **Runs view**: All benchmark runs with expandable per-benchmark statistics
- **Compare view**: Visual baseline vs. target comparison
- **Trends view**: Performance over time with confidence interval bands
- **Baselines view**: Manage named baselines

## Probes and GC Statistics

When `probes_path` is configured, each benchmark invocation also collects MMTk GC statistics:
- `GC`: Total GC count
- `majorGC`: Major GC count
- `time.stw`: Stop-the-world pause time
- `time.other`: Non-pause GC time

These metrics help diagnose **why** performance changed — e.g., fewer GCs, shorter pauses, or less time in GC overall.

## Running Benchmarks Outside mmtk-dev

For quick ad-hoc tests without mmtk-dev infrastructure, you can run DaCapo directly:

```fish
# Direct JVM execution (no probes/stats)
./openjdk/build/linux-x86_64-server-release/images/jdk/bin/java \
    -XX:+UseThirdPartyHeap \
    -XX:ThirdPartyHeapOptions=plan=GenImmix \
    -XX:MetaspaceSize=500M \
    -jar dacapo-23.11-MR2-chopin.jar fop

# With custom heap size
./openjdk/build/linux-x86_64-server-release/images/jdk/bin/java \
    -XX:+UseThirdPartyHeap \
    -XX:ThirdPartyHeapOptions=plan=GenImmix \
    -Xms96M -Xmx96M \
    -XX:MetaspaceSize=500M \
    -jar dacapo-23.11-MR2-chopin.jar fop

# Control GC thread count
./openjdk/build/linux-x86_64-server-release/images/jdk/bin/java \
    -XX:+UseThirdPartyHeap \
    -XX:ThirdPartyHeapOptions=plan=GenImmix \
    -XX:ParallelGCThreads=4 \
    -XX:MetaspaceSize=500M \
    -jar dacapo-23.11-MR2-chopin.jar fop

# List all available benchmarks
./openjdk/build/linux-x86_64-server-release/images/jdk/bin/java \
    -jar dacapo-23.11-MR2-chopin.jar -l

# Print nominal stats (minheap, etc.)
./openjdk/build/linux-x86_64-server-release/images/jdk/bin/java \
    -jar dacapo-23.11-MR2-chopin.jar -p fop
```

> **Important:** Always use `-XX:MetaspaceSize=500M` with MMTk since it doesn't support metaspace GC.

> **Important:** When running multiple DaCapo tests concurrently, create separate scratch directories and pass `--scratch-directory <dir>` to avoid output validation conflicts.

## Data Storage

All results are stored in a **SQLite database** (default: `~/.mmtk-dev/mmtk-dev.db`). The schema tracks:
- **Testbed**: Machine hardware info (CPU, cores, memory)
- **Build**: Git commits (mmtk-core + mmtk-openjdk), GC plan, build profile
- **Run**: Invocation count, heap multiplier, start/finish time
- **Result**: Per-benchmark × per-invocation execution times
- **Baseline**: Named reference points for comparisons

Build IDs are deterministic (hash of commit + plan), so the same code + plan always produces the same build ID.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "JDK not found" | Rebuild: `cd openjdk && make CONF=linux-x86_64-server-release ...` |
| "No default baseline" | Set one: `mmtk-dev set-baseline <name>` |
| Large confidence intervals | Increase invocations (`-i 20`), reduce system noise |
| `running-ng` errors | Use `--debug` flag to see generated config; check running-ng is installed |
| "No results found" | Check benchmark name spelling; check DaCapo jar path in config |
| Different results across machines | Only compare results from the same testbed |
