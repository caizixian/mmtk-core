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
invocations = 5
heap_multiplier = 3.0
iterations = 5
gc_threads = 32        # optional: -XX:ParallelGCThreads
app_threads = 32       # optional: DaCapo -t threads
```

- **`probes_path`**: When set, mmtk-dev auto-adds probes classpath, JVM args (`-Dprobes=RustMMTk`), and the DaCapo callback for MMTk statistics.
- **`heap_multiplier`**: Heap size = multiplier × minheap. Use 2–3× for stress-testing GC. The minheap values are defined in `mmtk-core/tools/mmtk-dev/src/mmtk_dev/config.py` (`DACAPO_MINHEAP` dict).
- **`iterations`**: DaCapo timing iterations per invocation. **Must be ≥ 5 for proper JIT warmup.** Setting this to 1 means no warmup — the JVM runs interpreted/C1-compiled code, making results meaningless. More iterations = more warmup before the timing iteration.
- **`invocations`**: Number of JVM invocations (separate JVM startups). Use ≥5 for dev iteration, ≥10 for meaningful statistical confidence, ≥20 for CI mode. Reduce invocations (not iterations) when short on time.
- **`gc_threads`**: Optional. Number of GC worker threads (`-XX:ParallelGCThreads`). If omitted, JVM auto-detects.
- **`app_threads`**: Optional. Number of DaCapo application threads (`-t`). If omitted, DaCapo auto-detects.

> [!CAUTION]
> **Never set `iterations = 1`.** This skips JIT warmup entirely. DaCapo runs `iterations` loops within each JVM invocation — only the last is the "timing iteration". With `iterations = 1`, there is no warmup and C2-compiled code may not be ready. Always use ≥ 5.

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
| `-n, --note` | Optional note to attach to the run | `-n "baseline before change"` |
| `--gc-threads` | GC worker threads (`-XX:ParallelGCThreads`) | `--gc-threads 32` |
| `--app-threads` | Application threads (DaCapo `-t`) | `--app-threads 32` |
| `--profile` | Build profile (`release`, `fastdebug`) | `--profile release` |
| `--debug` | Print the generated running-ng YAML config | `--debug` |

**Quick benchmarks for development iteration:**
```fish
# Single fast benchmark, 3 invocations — good for sanity checks
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop -i 3
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
> **Commit your changes before running benchmarks.** `mmtk-dev` captures the git commit hashes of `mmtk-core` and `mmtk-openjdk` for each run. If you run benchmarks with uncommitted changes, the recorded commit hash won't reflect the actual code being benchmarked. Always `git commit` (or `jj commit`) before proceeding to Step 5.
>
> **Do NOT revert files for baselines — checkout the commit.** When doing A/B comparisons, always `git checkout <commit>` to switch to the baseline code. Simply reverting files leaves the commit hash unchanged, so mmtk-dev records the wrong commit for the baseline run.

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
| `-b, --baseline` | Baseline name (**not** benchmark names!) | Default baseline |
| `--benchmarks` | Override benchmarks to run | Same as baseline |
| `--run-id` | Compare existing run (skip running) | |
| `--metric` | Metric(s) to compare: name, comma-separated, or `all` | |
| `--threshold` | Significance threshold | `0.02` (2%) |
| `-n, --note` | Optional note to attach to the comparison run | |
| `--gc-threads` | GC worker threads (`-XX:ParallelGCThreads`) | |
| `--app-threads` | Application threads (DaCapo `-t`) | |

> [!WARNING]
> **`-b` in `compare` means `--baseline`, not benchmarks.** Use `--benchmarks` to specify which benchmarks to run. For example:
> ```fish
> # CORRECT: compare against baseline "before-opt", running h2
> mmtk-dev compare -b before-opt --benchmarks h2
> # WRONG: this tries to find a baseline named "h2"
> mmtk-dev compare -b h2
> ```

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
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b lusearch -i 3
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
2. **No other work during benchmarks**: Do NOT run any other processes (file searches, builds, profiling prep, web searches, etc.) while benchmarks are executing. Any CPU activity introduces noise. Wait idle for benchmark completion.
3. **Sufficient invocations**: Use ≥10 for development, ≥20 for publishable results
4. **Consistent heap size**: Always use the same `--heap-multiplier` between baseline and comparison
5. **Same testbed**: Compare results from the same machine only
6. **Same plan and profile**: Don't compare GenImmix release against Immix fastdebug

### Choosing Heap Multiplier

The heap multiplier determines how much GC pressure is applied. Choose based on what you're measuring:

| Multiplier | GC Pressure | Best for |
|:----------:|:-----------:|----------|
| 1.5–2× | Very high | Testing GC-intensive optimizations (sweep, tracing, allocation under pressure) |
| 2–3× | High | Default for most GC optimizations |
| 5× | Low | Testing allocation fast-path and mutator-side changes; GC happens rarely |
| 10×+ | Minimal | Isolating mutator overhead from GC overhead |

> [!TIP]
> If you're optimizing sweep, tracing, or other GC-internal paths, use **2–3×** heap. At 5× heap, GC is so infrequent that sweep/trace time becomes unmeasurable noise. Conversely, if you're optimizing allocation or mutator overhead, use **5×+** to minimize GC interference.

### Iterative Optimization Pattern

```fish
# 1. Checkout the unmodified commit and build
git checkout <baseline-commit>  # or: jj edit <change-id>
cd openjdk && make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images

# 2. Run baseline from workspace root
cd ..
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop,lusearch,xalan -i 5 -n "baseline" # be more descriptive than just baseline
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev set-baseline before-opt

# 3. Switch to optimization commit and rebuild
cd mmtk-core && git checkout <opt-branch>  # or: jj edit <opt-change>
cd ../openjdk && make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images

# 4. Compare (this runs benchmarks + shows diff)
cd ..
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare -n "with optimization" # be more descriptive of what you changed

# 5. Compare additional metrics like STW time on the SAME run (no re-run needed!)
# IMPORTANT: use --run-id to compare an existing run with a different metric.
# Do NOT call `compare` without --run-id when you only want a different metric view.
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare --run-id <run-id> --metric time.stw
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare --run-id <run-id> --metric all
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
- `time.stw`: Stop-the-world pause time (ms)
- `time.other`: Non-pause GC time (ms)
- `total-work.count`, `total-work.time.total`, etc.: Work packet statistics

These metrics help diagnose **why** performance changed. Use `--metric` with `compare` to see them:

```fish
# Compare a specific metric on an EXISTING run (no re-run needed)
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare --run-id <run-id> --metric time.stw

# Compare all available metrics on an existing run
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare --run-id <run-id> --metric all
```

> [!IMPORTANT]
> **Always use `--run-id` when comparing a different metric on an existing run.** Without `--run-id`, `compare` will re-run all benchmarks from scratch, wasting time. The `--run-id` is printed at the end of every `run` or `compare` output.

The web dashboard also has a metric selector dropdown on the Compare page.

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
| Wrong commit hash in baseline | Use `git checkout`/`jj edit` to switch commits, don't revert files |
| GC optimization shows no improvement | Try tighter heap (2–3×); at 5× GC paths may be unmeasurable |
| `compare -b X` says "baseline not found" | `-b` means `--baseline` name, use `--benchmarks` for benchmarks |

## Autonomous Profiling-Driven Optimization Workflow

This section defines a structured, repeatable workflow for an agent to autonomously discover, implement, and validate performance optimizations. Follow this cycle for each optimization attempt.

> [!IMPORTANT]
> **Before starting any optimization work**, read all existing reports in `docs/` (e.g., `docs/nontemporal-zeroing-report.md`, `docs/genimmix-profiling-report.md`). These contain profiling data, benchmark results, and analysis from previous sessions that inform what has already been tried and what bottlenecks are known.
>
> **After completing a session**, you MUST update the **Lessons Learned** and **Current Known Bottlenecks** sections at the bottom of this file to reflect any new findings, failed approaches, or newly discovered optimization opportunities. This keeps future agents from repeating mistakes and helps them prioritize effectively.

### The Optimization Cycle

```
 1. Profile       →  2. Identify Bottleneck  →  3. Design Fix
      ↑                                              ↓
 6. Report        ←  5. Benchmark            ←  4. Implement
      ↓
 7. Next iteration (go to 1 or 3)
```

### Step 1: Profile

Generate profiling data to find where time is actually spent. Use async-profiler with collapsed stacks, then analyze with the `flamegraph_query` skill.

```fish
# Profile a benchmark with async-profiler (CPU, DWARF cstack for native frames)
./openjdk/build/linux-x86_64-server-release/images/jdk/bin/java \
    -XX:+UseThirdPartyHeap -XX:ThirdPartyHeapOptions=plan=GenImmix \
    -XX:MetaspaceSize=500M -Xms63m -Xmx63m \
    -agentpath:/path/to/libasyncProfiler.so=start,event=cpu,cstack=dwarf,file=profile.jfr \
    -jar dacapo-23.11-MR2-chopin.jar lusearch

# Convert to collapsed stacks for analysis
jfrconv --cpu -o collapsed profile.jfr > profile.collapsed

# Query with flamegraph_query skill
python3 .agents/skills/flamegraph_query/flamegraph_query.py profile.collapsed top -n 30
python3 .agents/skills/flamegraph_query/flamegraph_query.py profile.collapsed children "ProcessEdgesWork::do_work"
```

For instruction-level analysis (when function-level is too flat due to inlining):
```fish
# Build with fastdebug for DWARF debug info
DEBUG_LEVEL=fastdebug
cd openjdk && sh configure --disable-warnings-as-errors --with-debug-level=$DEBUG_LEVEL
make CONF=linux-x86_64-server-$DEBUG_LEVEL THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images

# Record with perf and annotate
perf record -e cpu-clock --call-graph dwarf -o perf.data -- ./openjdk/build/.../jdk/bin/java ...
perf annotate -i perf.data --symbol=<function>
```

### Step 2: Identify Bottleneck

Classify the bottleneck type to choose the right optimization strategy:

| Bottleneck Type | Profile Signature | Optimization Strategy |
|----------------|-------------------|----------------------|
| **Memory latency** | Stalls after `mov` loads, pointer chasing | Prefetching, batching, layout changes |
| **Cache contention** | `lock cmpxchg` stalls, CAS retries | Work partitioning, lock-free paths, reducing sharing |
| **Bandwidth** | `memset`/`memcpy` dominating | NT stores (on old CPUs), demand-zeroing, lazy init |
| **Scheduler overhead** | `futex`, `poll_schedulable_work` | Larger work packets, reduced synchronization |
| **Compute** | Arithmetic/logic instructions dominating | SIMD, algorithmic improvements |

> [!IMPORTANT]
> **Most GC hotspots are memory-latency-bound**, not compute-bound. If `perf annotate` shows stalls after loads (e.g., the instruction after a `mov` from memory has high sample count), the CPU is waiting for memory, not for computation. Computational optimizations (SIMD, caching loads, reducing arithmetic) will NOT help. Use prefetching or restructuring instead.

### Step 3: Design the Fix

**First, check what already exists in the codebase:**

- Search `mmtk-core` for existing implementations, benchmarks, or prototypes related to the optimization. The `benches/mock_bench/` directory contains microbenchmarks for prefetching, AMAC, etc.
- Check existing `docs/` reports for prior attempts at this optimization.
- Read the **Lessons Learned** section below to avoid repeating known-failed approaches.

**Then, research externally:**

- Search the web for papers on the specific technique (e.g., "prefetching garbage collection tracing", "non-temporal stores memory zeroing", "work stealing GC scheduling")
- Check if the technique has known limitations on modern hardware (e.g., ERMS on modern x86)
- Look for existing implementations in other GC frameworks (ZGC, Shenandoah, Go GC, etc.)
- Read papers fully and note their benchmarking methodology, hardware, and reported gains

**Assess the expected impact:**

1. **How much of total time does this bottleneck represent?**
   - GC workers are only 15-20% of total time in typical benchmarks
   - A 50% improvement within GC tracing saves only ~10% of total runtime
   - Mutation-side improvements (allocation, barriers) affect the other 80%, but require modifying compiler IR in the binding

2. **Is the optimization addressing the root cause?**
   - If the bottleneck is `memset`, check if the CPU's `memset` already uses NT stores (ERMS)
   - If the bottleneck is cache misses, prefetching helps; faster computation does not
   - If the bottleneck is lock contention, reducing critical section time helps; prefetching does not

3. **What could go wrong?**
   - Prefetching memory that's already cached wastes i-cache and bandwidth
   - NT stores for memory that's immediately reused forces cache re-fetch
   - Additional instructions in a hot loop can increase i-cache pressure
   - Modifying hot loops in `gc_work.rs` can have subtle perf effects — validate with microbenchmarks first

### Step 4: Implement

Follow these rules during implementation:

1. **Validate with microbenchmarks first** if one exists in `benches/mock_bench/` — this is faster than a full DaCapo run and catches obvious issues
2. **Commit changes before benchmarking** — `mmtk-dev` records the git commit hash
3. **Make minimal, focused changes** — one optimization per commit for clean A/B comparison
4. **Use `git checkout`/`jj edit` for baselines** — never revert files manually (commit hash must differ)
5. **Add comments citing the profiling data** that motivated the change
6. **Test correctness first** — run `java -XX:+UseThirdPartyHeap ... -jar dacapo.jar fop` to verify no crashes before committing

### Step 5: Benchmark

Use `mmtk-dev` for statistically rigorous comparison:

```fish
# If no baseline exists for the current config, create one:
# 1. Checkout the parent commit
cd mmtk-core && git checkout <parent-commit>
# 2. Build
cd ../openjdk && make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images
# 3. Run baseline
cd ..
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b lusearch,h2,fop -i 5 -n "baseline: describe parent state"
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev set-baseline <descriptive-name>

# Switch to optimization commit and rebuild
cd mmtk-core && git checkout <opt-branch>
cd ../openjdk && make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images

# Compare
cd ..
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare -n "describe the optimization"
```

**Reusing baselines:** If you revert to the exact baseline commit (verified via `git log`), you can reuse an existing baseline without re-running it. The same commit + plan + profile always produces the same build ID.

### Step 6: Report

**Every optimization attempt — successful or not — MUST be documented.**

Write a report in `docs/` with this structure:

```markdown
# [Optimization Name] Experiment Report

**Date:** YYYY-MM-DD
**Status:** ✅ Improvement / ❌ No improvement — reverted / ➡️ Neutral
**Commit:** `<hash>` (reverted in `<hash>` if applicable)

## Hypothesis
What profiling data motivated this? What did you expect to happen?

## Changes Made
What files were modified and how?

## Benchmark Results
Table with baseline vs. optimized, CI, diff%, status.

## Analysis
Why did it work or not work? What did you learn?

## Conclusions
What's the next step based on these results?
```

Commit the report with a descriptive message (e.g., `docs: add NT zeroing experiment report`).

### Step 7: Iterate

After each attempt, decide the next action:

| Result | Action |
|--------|--------|
| ✅ Improvement ≥ 2% | Keep the change. Re-profile to find the next bottleneck. |
| ➡️ Neutral (< 2%) | Revert unless the change simplifies code. Move to next bottleneck. |
| ❌ Regression | Revert immediately. Analyze why. Document in report. Move on. |
| High variance | Increase invocations. Check for system noise. Re-run. |

### Lessons Learned (from past optimization attempts)

These are documented outcomes that future agents should use to avoid repeating failed approaches:

1. **Non-temporal zeroing does NOT help on modern CPUs** (AMD Zen 3, Intel Ivy Bridge+).
   The standard `memset` already uses `rep stosb` with ERMS which internally does NT stores for large fills.
   See `docs/nontemporal-zeroing-report.md`.

2. **SIMD hole search regressed (+11.46%)** because `get_next_available_lines` scans only 2–16 bytes.
   SSE2 setup and `pmovmskb` FP→INT domain crossing (5 cycles on Zen 3) costs more than scalar code.
   See `docs/genimmix-profiling-report.md`.

3. **Cached mark states had no effect (−0.65%)** because `Ordering::Acquire` loads compile to plain `mov` on x86 (no fence). Saving 1–2 ns in a function that doesn't appear in the profile is unmeasurable.
   See `docs/genimmix-profiling-report.md`.

4. **The allocation fast path doesn't appear in Rust-side profiling** because it's JIT-compiled
   by C1/C2 using the barrier set assembler (`mmtkBarrierSetAssembler_x86.cpp` in `mmtk-openjdk`).
   It IS a valid optimization target, but requires modifying compiler IR in the OpenJDK binding.
   The allocation **slow path** (page acquisition + zeroing in Rust) is what shows up in `perf`/async-profiler.

5. **All GC hotspots are memory-latency-bound.** Computational optimizations (faster arithmetic, fewer branches, SIMD) do not help when the CPU is stalled on memory.

6. **Always validate with microbenchmarks before modifying production hot paths.** The `benches/mock_bench/` directory contains prefetching (`prefetch_tracing.rs`) and AMAC (`amac_tracing.rs`) benchmarks that model the tracing loop. Use these to validate prefetch distances, cache hints, and pipeline strategies before touching `gc_work.rs`.

8. **Two-stage edge+object prefetch regressed vs object-only** (-0.91% vs -2.66%). Slot buffers within work packets have good spatial locality (produced by scanning contiguous OopMap fields), so the HW prefetcher handles edge data. The extra `slot.load()` at `i+32` wastes load ports. Microbenchmarks use random DAGs that maximize cache misses, hiding this effect.
   See `docs/prefetch-tracing-report.md`.

7. **Software prefetching in the tracing loop works** (-2.66% geomean, -3.83% on h2). Prefetching object headers 16 slots ahead in `process_slots()` and 4 objects ahead in `ScanObjectsWork::do_work_common()` with NTA hint effectively hides memory latency. Validated with microbenchmarks first. Lusearch is neutral because it's scheduler-dominated, not tracing-dominated.
   See `docs/prefetch-tracing-report.md`.

9. **After prefetch, remaining GC bottlenecks are broadly distributed** with no single target offering >2% total runtime improvement. The bottleneck shifted from one dominant stall (29% pointer chasing) to many small targets: object copying 18.9%, side metadata 9.1%, CAS 9.1%, descriptor lookup 6.5%. Further gains require architectural changes (compound prefetch, scheduler redesign) or are fundamental work (memcpy).
   See `docs/genimmix-profiling-report.md` § Post-Prefetch Re-Profiling.

10. **Prefetching `descriptor_map` entries had no measurable impact** (h2 +0.84%, fop -0.94%, both in noise). During nursery GC, only a few chunks contain nursery objects, so the `descriptor_map[chunk_index]` entries stay warm in L2/L3 cache. The 6.5% profile self-time in `get_descriptor_for_address` is likely attributable to the computation/dispatch overhead rather than actual cache misses on the descriptor_map array. Reverted at `8111ea5663`.

11. **Always test one change at a time** with a proper A/B comparison against a known baseline. When stacking multiple commits (e.g. body prefetch + forwarding cache), you cannot attribute measured improvements to either change individually. Each experiment should have exactly one independent variable.

### Current Known Bottlenecks (from profiling)

Refer to `docs/genimmix-profiling-report.md` and `docs/prefetch-tracing-report.md` for full analysis. Key targets, ordered by potential impact:

#### Pre-Prefetch Bottlenecks (resolved or unchanged)

| Bottleneck | % of GC time | Root cause | Promising fix | Status |
|------------|-------------|------------|---------------|--------|
| ProcessEdgesWork pointer chasing | 29% → **13.4%** | Memory latency on slot/oop loads | Prefetching objects 16 ahead in `process_slots()` | ✅ Addressed (`929c46144d`, -2.66% geomean) |
| PlanScanObjects header stall | 53% → 8.4% | Cache miss loading compressed klass | Prefetching object headers 4 ahead during scan | ✅ Addressed (`929c46144d`) |

#### Post-Prefetch Bottlenecks (current state, from h2 re-profiling)

| Bottleneck | % of GC time | Total % | Root cause | Promising fix | Status |
|------------|-------------|---------|------------|---------------|--------|
| Object copying | 18.9% (self) | ~2.3% | Fundamental memcpy of nursery objects | Nursery sizing, copy strategy changes | ❌ Design-level |
| Side metadata access | 9.1% (self) | ~1.1% | Cache miss on metadata byte load | Compound prefetch (metadata + header) | ⚠️ Complex — needs per-space spec access |
| CopySpace CAS contention | 9.1% (self) | ~1.1% | Cache-line contention on forwarding bits | Work partitioning, reducing duplicate tracing | ❌ Requires scheduler-level changes |
| Space descriptor lookup | 6.5% (self) | ~0.8% | descriptor_map entries warm (few active chunks) | Prefetch tried — no effect (reverted `8111ea5663`) | ❌ Not a cache miss bottleneck |
| Vec reallocation | 3.6% (PEW %) | ~0.4% | nodes VectorQueue growing beyond capacity | Pre-allocate with larger initial capacity | ✅ Easy but tiny impact |
| Scheduler futex overhead | 52% (lusearch) | — | 32 GC threads competing for small work packets | Larger work packets, adaptive thread count | 🔲 Only matters with many GC threads + small heap |
| Allocation fast path (JIT) | Not visible in Rust profiles | — | JIT-compiled in `mmtkBarrierSetAssembler_x86.cpp` | C2 IR optimization in binding | 🔲 Requires binding changes |

> **NOTE**: After prefetching, no single remaining target offers >2% total improvement. The bottleneck is now broadly distributed across fundamental work (copying, CAS, metadata).

### Session-End Checklist

> [!CAUTION]
> **You MUST complete this checklist at the end of every optimization session:**
>
> 1. **Write a report** in `docs/` for every optimization attempted (successful or not) and `git commit` it
> 2. **Update Lessons Learned** above with any new findings or failed approaches
> 3. **Update Current Known Bottlenecks** if profiling revealed new data or resolved existing items
> 4. **Commit this skill file** with `git add .agents/skills/performance-benchmarking/SKILL.md && git commit`
>
> Failure to update this file means the next agent will repeat your mistakes.

