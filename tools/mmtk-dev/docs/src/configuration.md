# Configuration

mmtk-dev uses a `.mmtk-dev.toml` file for workspace configuration. Place it at the root of your workspace (next to `mmtk-core/`, `mmtk-openjdk/`, `openjdk/`).

## Example Configuration

```toml
[workspace]
mmtk_core = "./mmtk-core"
mmtk_openjdk = "./mmtk-openjdk"
openjdk = "./openjdk"
dacapo_jar = "./dacapo-23.11-MR2-chopin.jar"
dacapo_suite = "dacapochopin"
probes_path = "./probes"
db_path = "~/.mmtk-dev/mmtk-dev.db"

[testbed]
id = "my-workstation"
name = "My Workstation"

[defaults]
plan = "GenImmix"
benchmarks = ["fop", "lusearch", "xalan"]
invocations = 10
heap_multiplier = 3.0
iterations = 6

[api]
url = "http://localhost:8080"
```

## Fields

### `[workspace]`

| Field | Description | Default |
|-------|-------------|---------|
| `mmtk_core` | Path to mmtk-core repo | `.` |
| `mmtk_openjdk` | Path to mmtk-openjdk repo | `../mmtk-openjdk` |
| `openjdk` | Path to openjdk repo | `../openjdk` |
| `dacapo_jar` | Path to DaCapo benchmark jar | (detected from suite) |
| `dacapo_suite` | Benchmark suite name (`dacapo2006`, `dacapochopin`, etc.) | `dacapo2006` |
| `probes_path` | Path to [probes](https://github.com/anupli/probes) repo (enables MMTk stats) | (none) |
| `db_path` | SQLite database path | `~/.mmtk-dev/mmtk-dev.db` |

### `[testbed]`

| Field | Description | Default |
|-------|-------------|---------|
| `id` | Unique testbed identifier | (auto-detected from hostname) |
| `name` | Human-readable testbed name | (auto-detected from hostname) |

### `[defaults]`

| Field | Description | Default |
|-------|-------------|---------|
| `plan` | Default GC plan | `GenImmix` |
| `benchmarks` | Default benchmark list | `["fop"]` |
| `invocations` | Default invocation count | `10` |
| `heap_multiplier` | Default heap multiplier | `3.0` |
| `iterations` | DaCapo timing iterations | `6` |
