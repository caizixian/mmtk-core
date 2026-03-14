# Workflow

mmtk-dev supports two primary workflows: **individual researcher** use and **CI integration**.

## Individual Researcher Workflow

```
┌─────────────┐    ┌──────────┐    ┌─────────────┐    ┌───────────┐
│ Build MMTk  │───▶│ Run      │───▶│ Set         │───▶│ Iterate   │
│ (baseline)  │    │ Benchmarks│    │ Baseline    │    │ & Compare │
└─────────────┘    └──────────┘    └─────────────┘    └───────────┘
```

1. **Build** your baseline version of mmtk-core + mmtk-openjdk
2. **Run** benchmarks: `mmtk-dev run -b fop,lusearch,xalan -i 10`
3. **Set baseline**: `mmtk-dev set-baseline before-optimization`
4. **Iterate**: make changes, rebuild, then `mmtk-dev compare`

Each run is recorded with full provenance:
- Git commits for mmtk-core and mmtk-openjdk
- GC plan and build profile
- Testbed hardware info
- Per-invocation execution times

## CI Integration Workflow

```
┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐
│ PR Opens │───▶│ Build PR │───▶│ mmtk-dev │───▶│ Report   │
│          │    │ + Master │    │ ci       │    │ to PR    │
└──────────┘    └──────────┘    └──────────┘    └──────────┘
```

The `ci` command is designed for GitHub Actions:

```bash
mmtk-dev ci --plans GenImmix,Immix --output-format markdown --alert-threshold 0.02
```

Key features:
- Runs benchmarks for multiple plans
- Compares against the default baseline
- Outputs markdown tables suitable for PR comments
- Exits non-zero if regressions exceed the threshold

## Data Model

The SQLite database tracks the following relationships:

```
Testbed ◀───── Run ─────▶ Build
                │
                ├── Result (per benchmark × invocation)
                │     └── Metric (per result)
                │
                └── Baseline (named reference point)
```
