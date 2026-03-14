# API Reference

The mmtk-dev API server provides a REST API for programmatic access to benchmark data.

## Starting the Server

```bash
uv run mmtk-dev server --port 8080
```

## Endpoints

### Health

```
GET /api/health
```

Returns `{"status": "ok", "version": "0.1.0"}`.

### Builds

```
GET /api/builds                    # List builds (query: ?limit=50)
GET /api/builds/{build_id}         # Get build details
```

### Runs

```
GET /api/runs                      # List runs (?limit=50&build_id=...&testbed_id=...)
GET /api/runs/{run_id}             # Get run details
GET /api/runs/{run_id}/results     # Get results with computed statistics
```

### Baselines

```
GET /api/baselines                 # List all baselines
GET /api/baselines/{name}          # Get baseline by name
```

### Compare

```
GET /api/compare                   # Compare runs
```

Query parameters:
- `baseline` — baseline name (default: default baseline)
- `run_id` — target run ID (default: latest run)
- `threshold` — significance threshold (default: 0.02)

Returns per-benchmark comparisons with diffs and a geometric mean.

### Testbeds

```
GET /api/testbeds                  # List testbed machines
```
