---
name: flamegraph-query
description: Query and analyze collapsed-stack flamegraph data from async-profiler (jfrconv). Supports summary, top functions, callers/callees, filtering, and diffing profiles without needing a browser.
---

# Flamegraph Query Tool

This skill provides a CLI tool for querying collapsed-stack flamegraph data produced by async-profiler's `jfrconv`. It allows you to analyze profiling results entirely from the command line — no browser needed.

## Tool Location

```
mmtk-core/.agents/skills/flamegraph_query/
├── SKILL.md              # This file
└── flamegraph_query.py   # CLI tool
```

## Generating Collapsed Stacks from async-profiler

async-profiler records JFR files. Convert them to collapsed stacks using `jfrconv`:

```bash
# Record a JFR profile (via agentpath)
java -agentpath:/path/to/libasyncProfiler.so=start,event=cpu,interval=1000000,cstack=dwarf,file=profile.jfr ...

# Convert JFR to collapsed stacks
/path/to/jfrconv --cpu -o collapsed profile.jfr profile_collapsed.txt

# Per-thread flamegraph HTML (for manual inspection)
/path/to/jfrconv --cpu -t -o html profile.jfr profile_threads.html
```

## Quick Reference

All examples assume collapsed stacks have been generated at `profile.txt`.

### Summary (thread/category breakdown)

```bash
python3 .agents/skills/flamegraph_query/flamegraph_query.py summary profile.txt
```

Output includes: total samples, GC worker vs app thread breakdown, allocation slow path percentage, top MMTk leaf functions.

### Top functions

```bash
# Top 20 self (leaf) functions — where the CPU actually stopped
python3 .agents/skills/flamegraph_query/flamegraph_query.py top profile.txt

# Top 20 inclusive functions — who is on the stack the most
python3 .agents/skills/flamegraph_query/flamegraph_query.py top --mode inclusive profile.txt

# Top functions only in GC worker threads
python3 .agents/skills/flamegraph_query/flamegraph_query.py top --thread gc profile.txt

# Top functions only in application threads
python3 .agents/skills/flamegraph_query/flamegraph_query.py top --thread app profile.txt

# Top functions matching a pattern
python3 .agents/skills/flamegraph_query/flamegraph_query.py top --pattern trace_object profile.txt

# Change number of results
python3 .agents/skills/flamegraph_query/flamegraph_query.py top -n 30 --thread gc --mode inclusive profile.txt
```

### Callers / Callees

```bash
# Who calls trace_object? (immediate parent frames)
python3 .agents/skills/flamegraph_query/flamegraph_query.py callers trace_object profile.txt

# What does trace_object call? (immediate child frames)
python3 .agents/skills/flamegraph_query/flamegraph_query.py callees trace_object profile.txt

# Who calls alloc_slow_inline?
python3 .agents/skills/flamegraph_query/flamegraph_query.py callers alloc_slow_inline profile.txt
```

### Filter stacks by pattern

```bash
# Show all stacks containing "alloc" — leaf and inclusive breakdown
python3 .agents/skills/flamegraph_query/flamegraph_query.py filter alloc profile.txt

# Filter to GC thread stacks containing "metadata"
python3 .agents/skills/flamegraph_query/flamegraph_query.py filter metadata --thread gc profile.txt
```

### Diff two profiles (A/B comparison)

```bash
# Compare baseline vs optimized profile
python3 .agents/skills/flamegraph_query/flamegraph_query.py diff baseline.txt optimized.txt

# Top 30 changes
python3 .agents/skills/flamegraph_query/flamegraph_query.py diff baseline.txt optimized.txt -n 30
```

Output shows per-function percentage change, sorted by absolute delta.

## Thread Classification

Stacks are classified by thread type:

| Thread | Detection | Description |
|--------|-----------|-------------|
| `gc` | Contains `start_worker` in early frames | MMTk GC worker threads |
| `alloc` | Contains `alloc_slow_inline` | App threads in allocation slow path |
| `app` | Everything else | Application threads doing normal work |

## Interpreting Results

### Self vs Inclusive

- **Self (leaf)**: Where the CPU was actually sampled — the instruction being executed. This tells you what's *actually slow*.
- **Inclusive**: All frames on the stack. This tells you what *causes* slowness throughout its call tree.

### Common hotspot patterns

| Pattern | What it means |
|---------|---------------|
| High self% on `lock cmpxchg` / CAS | Cache-line contention between threads |
| High self% on `mov` / memory load | Memory latency stall (cache miss) |
| High self% on `syscall` | Kernel boundary crossing (mmap, futex) |
| High self% on `__memset` / `__memcpy` | Bulk memory operations (page zeroing, object copy) |
| High inclusive% but low self% | Function is a coordinator, not a bottleneck itself |

## Integration with perf annotate

For instruction-level analysis of hot functions identified by this tool, use:

```bash
perf annotate -i perf.data --stdio -s "function::name"
```

This shows per-instruction sample percentages with inlined source code context, complementing the function-level view from collapsed stacks.
