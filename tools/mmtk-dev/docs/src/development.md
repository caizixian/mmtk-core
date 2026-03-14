# Development

This page covers the workflow for contributors working on mmtk-dev itself.

## Tech Stack

| Component | Technology | Version | Purpose |
|-----------|-----------|---------|---------|
| Language | Python | ≥ 3.11 | Backend, CLI, stats, DB |
| Package manager | [uv](https://docs.astral.sh/uv/) | latest | Dependency management & virtualenv |
| CLI framework | [Click](https://click.palletsprojects.com/) | ≥ 8 | CLI commands (`mmtk-dev run`, `compare`, …) |
| Web framework | [FastAPI](https://fastapi.tiangolo.com/) | ≥ 0.100 | REST API backend |
| ASGI server | [Uvicorn](https://www.uvicorn.org/) | ≥ 0.20 | Serves the FastAPI app |
| Database | SQLite (stdlib) | — | Per-invocation result storage, WAL mode |
| Statistics | NumPy + SciPy | latest | Mean, CI, outlier removal, geomean |
| Runner | [running-ng](https://github.com/nicebench/running-ng) | latest | Benchmark execution engine (subprocess) |
| Terminal UI | [Rich](https://rich.readthedocs.io/) | ≥ 13 | Pretty CLI output, tables, progress |
| Config parsing | PyYAML + tomllib | — | YAML config gen, TOML workspace config |
| Frontend lang | TypeScript | ≥ 5 | Typed React components |
| Frontend runtime | Node.js | ≥ 18 | Build toolchain |
| Frontend framework | React | 18 | Component-based UI |
| Bundler | [Parcel](https://parceljs.org/) | 2 | Zero-config TypeScript/JSX bundling |
| CSS framework | [Tailwind CSS](https://tailwindcss.com/) | 3 | Utility-first styling |
| Documentation | [mdbook](https://rust-lang.github.io/mdBook/) | latest | This documentation site |
| Linter | [Ruff](https://docs.astral.sh/ruff/) | latest | Python lint + format |
| Type checker | [mypy](https://mypy-lang.org/) | latest | Static type checking |
| Tests | [pytest](https://pytest.org/) | latest | Unit tests |

## Prerequisites

- **Python 3.11+** with [uv](https://docs.astral.sh/uv/)
- **Node.js 18+** with npm (for the web frontend)
- [mdbook](https://rust-lang.github.io/mdBook/) (for documentation)

## Setup

```bash
cd mmtk-core/tools/mmtk-dev

# Install Python dependencies
uv sync

# Install frontend dependencies
cd frontend && npm install && cd ..
```

## Running the Server

```bash
# Build frontend + start server (recommended)
uv run mmtk-dev server

# Skip frontend build (if already built)
uv run mmtk-dev server --skip-build

# Custom port and host
uv run mmtk-dev server --port 9090 --host 0.0.0.0
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `MMTK_DEV_DB_PATH` | `~/.mmtk-dev/mmtk-dev.db` | Path to the SQLite database. Set by `server` command, or manually for API-only usage. |

## Running Tests

```bash
# Run all Python tests (65 tests across 4 files)
uv run pytest tests/ -v

# Run a specific test file
uv run pytest tests/test_db.py -v
uv run pytest tests/test_parser.py -v
uv run pytest tests/test_stats.py -v
uv run pytest tests/test_api.py -v
```

## Linting & Formatting

```bash
# Lint check
uv run ruff check src/ tests/

# Auto-fix lint issues
uv run ruff check --fix src/ tests/

# Format check
uv run ruff format --check src/ tests/

# Auto-format
uv run ruff format src/ tests/
```

## Type Checking

```bash
uv run mypy src/
```

## Frontend Development

The frontend is a React + TypeScript app built with Parcel and styled with Tailwind CSS v3.

```bash
cd frontend

# Install dependencies
npm install

# Development server (hot reload on port 3000)
# Point API calls to the backend by starting
# 'uv run mmtk-dev server --skip-build' on port 8080 separately
npm run dev

# Production build (outputs to frontend/dist/)
npm run build
```

### Frontend ↔ Backend Interface

The frontend TypeScript interfaces in `api.ts` must match the JSON shapes returned by the FastAPI backend in `api/api.py`. Key fields to keep in sync:

| Frontend (api.ts) | Backend (Python) | Notes |
|-------------------|------------------|-------|
| `Stats.stdev` | `compute_statistics()["stdev"]` | Standard deviation |
| `Stats.mean` | `compute_statistics()["mean"]` | Arithmetic mean |
| `Stats.ci` | `compute_statistics()["ci"]` | Confidence interval half-width |
| `Run.finished_at` | `run` table `finished_at` column | Completion timestamp |
| `CompareResult.baseline_name` | `bl["id"]` | Baseline name = baseline primary key |

The backend also returns extra fields (`n_outliers`, `mean_no_outliers`, `ci_no_outliers`, `heap_multiplier`, `running_ng_id`, etc.) that the frontend currently ignores.

### Frontend Structure

```
frontend/src/
├── index.html          # Entry HTML
├── index.tsx           # React root
├── index.css           # Tailwind entry
├── api.ts              # Typed API client + interfaces
├── App.tsx             # Main app with sidebar nav
└── components/
    ├── RunsView.tsx    # Runs table + detail panel
    ├── CompareView.tsx # Baseline vs target comparison
    ├── TrendsView.tsx  # Canvas performance chart
    └── BaselinesView.tsx
```

### Tailwind Configuration

The `tailwind.config.js` `content` field must include `ts` and `tsx` extensions to avoid Tailwind purging classes from the production build:

```js
content: ['./src/**/*.{html,js,jsx,ts,tsx}'],
```

Custom tokens (`surface`, `border`) define the dark-mode color palette. See `tailwind.config.js` for the full theme.

## Documentation

```bash
cd docs

# Live preview with hot reload
mdbook serve --open

# Build static HTML
mdbook build
```

## Pre-commit Checklist

Before committing, ensure all checks pass:

```bash
uv run ruff check src/ tests/
uv run ruff format --check src/ tests/
uv run mypy src/
uv run pytest tests/ -q
```

## Project Layout

| Directory | Language | Purpose |
|-----------|----------|---------|
| `src/mmtk_dev/cli/` | Python | Click CLI commands (`run`, `compare`, `set-baseline`, `ci`, `server`) |
| `src/mmtk_dev/api/` | Python | FastAPI REST backend + static file serving |
| `src/mmtk_dev/db/` | Python | SQLite schema (6 tables) + query layer |
| `src/mmtk_dev/runner/` | Python | running-ng integration, log parser, config generator |
| `src/mmtk_dev/stats/` | Python | Statistical analysis (mean, CI, z-score outlier removal, geomean) |
| `src/mmtk_dev/config.py` | Python | `.mmtk-dev.toml` workspace config + git/testbed detection |
| `src/mmtk_dev/web/` | — | Static file directory for built frontend assets |
| `frontend/src/` | TypeScript/React | Web dashboard (Parcel + Tailwind) |
| `tests/` | Python | pytest unit tests (db, parser, stats, API) |
| `docs/src/` | Markdown | mdbook documentation |

## Known Limitations

- **MMTk metrics partially wired:** The `metric` table and `insert_metrics()` function exist but are not yet connected. MMTk statistics are parsed from logs but not stored in the DB.
- **No `/api/trends` endpoint:** The frontend's TrendsView synthesizes trend data client-side by hitting `/api/runs` + `/api/runs/{id}/results`.
- **Static file mount at import time:** The `app.mount("/static", ...)` call in `api.py` runs when the module is imported. If `web/dist/` is created after server start, a restart is needed.
