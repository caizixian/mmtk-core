# CLI Reference

## `mmtk-dev run`

Run benchmarks and record results.

```
mmtk-dev run [OPTIONS]
```

| Option | Description | Default |
|--------|-------------|---------|
| `-b, --benchmarks` | Comma-separated benchmark names, or `all` | From config |
| `-p, --plan` | GC plan (e.g. `GenImmix`, `Immix`) | From config |
| `-i, --invocations` | Number of invocations | From config |
| `-m, --heap-multiplier` | Heap size as multiplier of minheap | From config |
| `--iterations` | DaCapo timing iterations per invocation | From config |
| `--profile` | Build profile (`release`, `fastdebug`, etc.) | `release` |
| `--log-dir` | Directory for logs (default: temp) | Auto |
| `--db` | Path to SQLite database | From config |

## `mmtk-dev set-baseline`

Mark a run as a named baseline for future comparisons.

```
mmtk-dev set-baseline NAME [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--run-id` | Run ID to use (default: latest run) |
| `--no-default` | Don't set as the default baseline |
| `-d, --description` | Description of this baseline |

## `mmtk-dev compare`

Compare current build against a baseline.

```
mmtk-dev compare [OPTIONS]
```

| Option | Description | Default |
|--------|-------------|---------|
| `-b, --baseline` | Baseline name to compare against | Default baseline |
| `--benchmarks` | Benchmarks to run | Same as baseline |
| `--run-id` | Compare existing run (skip running) | |
| `--threshold` | Threshold for significant change | `0.02` (2%) |

## `mmtk-dev ci`

Run benchmarks in CI mode with regression detection.

```
mmtk-dev ci [OPTIONS]
```

| Option | Description | Default |
|--------|-------------|---------|
| `--plans` | Comma-separated GC plans | `GenImmix` |
| `--output-format` | Output format (`rich` or `markdown`) | `rich` |
| `--alert-threshold` | Regression threshold | `0.02` (2%) |
| `--core-commit` | Override mmtk-core commit SHA | Auto-detected |
| `--core-branch` | Override mmtk-core branch | Auto-detected |

Exits with non-zero code if regressions are detected.

## `mmtk-dev server`

Start the API server and web dashboard.

```
mmtk-dev server [OPTIONS]
```

| Option | Description | Default |
|--------|-------------|---------|
| `--port` | Port to listen on | `8080` |
| `--host` | Host to bind to | `127.0.0.1` |
