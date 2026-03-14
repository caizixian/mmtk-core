# Development

This page covers the workflow for contributors working on mmtk-dev itself.

## Prerequisites

- Python 3.11+ with [uv](https://docs.astral.sh/uv/)
- Node.js 18+ with npm (for the web frontend)
- [mdbook](https://rust-lang.github.io/mdBook/) (for documentation)

## Setup

```bash
cd mmtk-core/tools/mmtk-dev

# Install Python dependencies
uv sync

# Install frontend dependencies
cd frontend && npm install && cd ..
```

## Running Tests

```bash
# Run all Python tests
uv run pytest tests/ -v

# Run a specific test file
uv run pytest tests/test_db.py -v
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

The frontend is a React + TypeScript app built with Parcel and styled with Tailwind CSS.

```bash
cd frontend

# Install dependencies
npm install

# Development server (hot reload on port 3000)
# Point API calls to the backend by starting
# 'uv run mmtk-dev server --skip-build' on port 8080 separately
npm run dev

# Production build (outputs to src/mmtk_dev/web/dist/)
npm run build
```

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

## Running the Server

```bash
# Build frontend + start server (recommended)
uv run mmtk-dev server

# Skip frontend build (if already built)
uv run mmtk-dev server --skip-build

# Custom port and host
uv run mmtk-dev server --port 9090 --host 0.0.0.0
```

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
| `src/mmtk_dev/cli/` | Python | Click CLI commands |
| `src/mmtk_dev/api/` | Python | FastAPI REST backend |
| `src/mmtk_dev/db/` | Python | SQLite schema + queries |
| `src/mmtk_dev/runner/` | Python | running-ng integration |
| `src/mmtk_dev/stats/` | Python | Statistical analysis |
| `frontend/src/` | TypeScript/React | Web dashboard |
| `tests/` | Python | pytest unit tests |
| `docs/src/` | Markdown | mdbook documentation |
