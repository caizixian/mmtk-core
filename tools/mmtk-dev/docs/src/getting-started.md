# Getting Started

## Prerequisites

- Python 3.11+
- [uv](https://docs.astral.sh/uv/) package manager
- A built OpenJDK with MMTk (see the main [MMTk guide](https://docs.mmtk.io))
- DaCapo Chopin benchmark jar (`dacapo-23.11-MR2-chopin.jar`)
- [probes](https://github.com/anupli/probes) built for MMTk statistics collection

## Installation

```bash
cd mmtk-core/tools/mmtk-dev
uv sync
```

This installs mmtk-dev and all dependencies, including `running-ng` for benchmark execution.

## Setup

### 1. Build probes

The [probes](https://github.com/anupli/probes) project provides Java callbacks that trigger MMTk to emit per-invocation GC statistics. Without probes, benchmarks still run, but no GC metrics are collected.

```bash
cd /path/to/probes
make
```

This produces `out/probes.jar` and `out/librust_mmtk_probe.so`.

> **Note:** The Makefile assumes `JAVA8_HOME` points to a JDK with `javac`. If your system JDK is in a different location, either set `JAVA8_HOME` explicitly or edit the Makefile.

### 2. Create `.mmtk-dev.toml`

Place a `.mmtk-dev.toml` at your workspace root (the directory containing `mmtk-core/`, `mmtk-openjdk/`, `openjdk/`):

```toml
[workspace]
mmtk_core = "./mmtk-core"
mmtk_openjdk = "./mmtk-openjdk"
openjdk = "./openjdk"
dacapo_jar = "./dacapo-23.11-MR2-chopin.jar"
dacapo_suite = "dacapochopin"
probes_path = "./probes"
db_path = "~/.mmtk-dev/mmtk-dev.db"

[defaults]
plan = "GenImmix"
benchmarks = ["fop"]
invocations = 10
heap_multiplier = 3.0
iterations = 1
```

Key fields:
- **`dacapo_suite`**: Must match a running-ng suite name (e.g. `dacapochopin`).
- **`dacapo_jar`**: Path to the DaCapo jar. The suite's default path is overridden with this.
- **`probes_path`**: Path to the built probes repo. When set, mmtk-dev automatically adds probes classpath, JVM args (`-Dprobes=RustMMTk`), and the DaCapo callback.

See [Configuration](./configuration.md) for the full reference.

### 3. Build OpenJDK with MMTk

```bash
cd openjdk
sh configure --disable-warnings-as-errors --with-debug-level=release
make CONF=linux-x86_64-server-release THIRD_PARTY_HEAP=$PWD/../mmtk-openjdk/openjdk images
```

## Typical Workflow

```
┌─────────────┐    ┌───────────┐    ┌─────────────┐    ┌───────────┐
│ Build MMTk  │───▶│   Run     │───▶│    Set      │───▶│ Iterate   │
│ (baseline)  │    │ Benchmarks│    │  Baseline   │    │ & Compare │
└─────────────┘    └───────────┘    └─────────────┘    └───────────┘
```

1. **Build** your baseline version of mmtk-core + mmtk-openjdk
2. **Run** benchmarks: `mmtk-dev run -b fop -i 10`
3. **Set baseline**: `mmtk-dev set-baseline before-optimization`
4. **Iterate**: make changes, rebuild, then `mmtk-dev compare`

## First Run

All commands below assume you are in the workspace root (the directory containing `.mmtk-dev.toml`).

### 1. Run benchmarks

Run the `fop` benchmark with 2 invocations:

```bash
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev run -b fop -i 2 --iterations 1
```

Sample output:

```
MMTk Performance Run
  Core:    main @ 5232b9c0
  Binding: master @ b692c5a2
  Plan:    GenImmix
  Benchmarks: fop
  Invocations: 2, Heap: 3.0x, Iterations: 1

  Run ID:  8342b0f11f18
  Build:   ca60a2db44f1d28b

Running benchmarks...
                       Run Results
┏━━━━━━━━━━━┳━━━━━━━━━━━┳━━━━━━━━━┳━━━━━━━━━━━━━┳━━━━━━━━┓
┃ Benchmark ┃ Mean (ms) ┃     ±CI ┃ Median (ms) ┃ Passed ┃
┡━━━━━━━━━━━╇━━━━━━━━━━━╇━━━━━━━━━╇━━━━━━━━━━━━━╇━━━━━━━━┩
│ fop       │    3262.5 │ ±1429.4 │      3262.5 │      2 │
└───────────┴───────────┴─────────┴─────────────┴────────┘

✓ Run 8342b0f11f18 completed
  To set as baseline: mmtk-dev set-baseline <name> --run-id 8342b0f11f18
  To compare:         mmtk-dev compare --baseline <name>
```

This will:
- Auto-detect your machine as a testbed
- Record git commits from mmtk-core and mmtk-openjdk
- Execute benchmarks via `running-ng` with probes (if configured)
- Parse results and store them in SQLite
- Capture MMTk GC statistics (`GC`, `majorGC`, `time.stw`, `time.other`, etc.) per invocation

> **Tip:** Add `--debug` to see the generated running-ng YAML config:
> ```bash
> mmtk-dev run -b fop -i 2 --iterations 1 --debug
> ```

### 2. Set a baseline

Mark the current run as your reference point:

```bash
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev set-baseline master
```

Sample output:

```
✓ Baseline 'master' set
  Run:     8342b0f11f18
  Commit:  5232b9c0
  Plan:    GenImmix
  Default: yes

  Compare against it: mmtk-dev compare
```

### 3. Make changes and compare

After modifying mmtk-core, rebuild OpenJDK, then compare:

```bash
uv run --project mmtk-core/tools/mmtk-dev mmtk-dev compare
```

This runs the same benchmarks and shows a comparison table:

```
Comparing against baseline "master" (commit 5232b9c0, GenImmix)

┏━━━━━━━━━━━┳━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━┳━━━━━━━━┳━━━━━━━━━━━━━┓
┃ Benchmark ┃  Baseline (ms) ┃   Current (ms) ┃   Diff ┃   Status    ┃
┡━━━━━━━━━━━╇━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━━━━╇━━━━━━━━╇━━━━━━━━━━━━━┩
│ fop       │ 3262.5 ±1429.4 │ 3100.0 ±500.0  │ -4.98% │ ✅ faster   │
└───────────┴────────────────┴────────────────┴────────┴─────────────┘

Geometric mean: -4.98% (improvement)
```

You can also compare an existing run without re-running benchmarks:

```bash
mmtk-dev compare --run-id <run-id>
```

### Running multiple benchmarks

Use comma-separated names to run several benchmarks:

```bash
mmtk-dev run -b fop,lusearch,xalan -i 10
```
