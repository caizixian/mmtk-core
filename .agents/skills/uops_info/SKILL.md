---
name: uops_info
description: Query x86/x64 instruction performance data (throughput, latency, uops, ports) from uops.info XML database using the uops_query.py CLI tool
---

# uops.info Instruction Performance Query

This skill provides access to detailed x86/x64 instruction performance data from [uops.info](https://uops.info), covering 28 microarchitectures (Intel Conroe through Alder Lake, AMD Zen+ through Zen 4) and 16877 instruction forms across 97 ISA extensions.

## Tool Location

The script and data are colocated with this skill:

```
mmtk-core/.agents/skills/uops_info/
├── SKILL.md          # This file
├── uops_query.py     # CLI tool
└── instructions.xml  # Auto-downloaded on first run (~110 MB, gitignored)
```

## Quick Reference

All commands below assume you are in the `mmtk-core` directory. Adjust the relative path as needed.

### Search for an instruction

```bash
# Search by mnemonic (substring match, case-insensitive)
python3 .agents/skills/uops_info/uops_query.py search MOV

# Filter to specific architecture(s)
python3 .agents/skills/uops_info/uops_query.py search PREFETCHW --arch SKL,ZEN4

# Exact mnemonic match to avoid partial hits (e.g. ADD not matching ADDPS)
python3 .agents/skills/uops_info/uops_query.py search ADD --exact --arch ICL

# Filter by ISA extension
python3 .agents/skills/uops_info/uops_query.py search VMOVDQU --extension AVX512EVEX

# JSON output for programmatic consumption
python3 .agents/skills/uops_info/uops_query.py --json search ADCX --arch SKL
```

### Get details for a specific instruction form

```bash
python3 .agents/skills/uops_info/uops_query.py info PREFETCHW_0F0Dr1
python3 .agents/skills/uops_info/uops_query.py info ADCX_GPR64q_GPR64q --arch SKL
```

### List available architectures and extensions

```bash
python3 .agents/skills/uops_info/uops_query.py list-arch
python3 .agents/skills/uops_info/uops_query.py list-ext
```

## Auto-Download

The `instructions.xml` file (~110 MB) is **automatically downloaded** from `https://uops.info/instructions.xml` on the first run. It is gitignored so it won't be committed.

## Architecture Codes

| Code   | Name           | Code   | Name           |
|--------|----------------|--------|----------------|
| CON    | Conroe         | ICL    | Ice Lake       |
| WOL    | Wolfdale       | TGL    | Tiger Lake     |
| NHM    | Nehalem        | RKL    | Rocket Lake    |
| WSM    | Westmere       | ADL-P  | Alder Lake-P   |
| SNB    | Sandy Bridge   | ADL-E  | Alder Lake-E   |
| IVB    | Ivy Bridge     | BNL    | Bonnell        |
| HSW    | Haswell        | AMT    | Airmont        |
| BDW    | Broadwell      | GLM    | Goldmont       |
| SKL    | Skylake        | GLP    | Goldmont Plus  |
| SKX    | Skylake-X      | TRM    | Tremont        |
| KBL    | Kaby Lake      | ZEN+   | AMD Zen+       |
| CFL    | Coffee Lake    | ZEN2   | AMD Zen 2      |
| CNL    | Cannon Lake    | ZEN3   | AMD Zen 3      |
| CLX    | Cascade Lake   | ZEN4   | AMD Zen 4      |

## Interpreting the Output

### Throughput (cycles per instruction)

- **TP(loop)**: Measured throughput in a loop (includes loop overhead)
- **TP(unrl)**: Measured throughput with unrolled instructions (no loop overhead)
- **TP(port)**: Computed throughput from port usage (theoretical peak)
- Lower values = better throughput. A value of `0.50` means 2 instructions per cycle.

### Micro-ops (μops)

- **μops**: Number of micro-ops the instruction decodes to
- Fewer μops = less frontend pressure

### Ports

Port notation like `1*p06+1*p23` means: 1 μop on port 0 or 6, plus 1 μop on port 2 or 3.

### Latency

Format: `op1→op1: 1c` means operand 1 to operand 1 latency is 1 cycle.
For memory operands: `addr=6c mem=≤3c` means 6 cycles from address, ≤3 cycles from memory value.

## JSON Output Schema

When using `--json`, each instruction result contains:

```json
{
  "asm": "ADCX",
  "string": "ADCX (R64, R64)",
  "iclass": "ADCX",
  "iform": "ADCX_GPR64q_GPR64q",
  "isa_set": "ADOX_ADCX",
  "extension": "ADOX_ADCX",
  "summary": "Unsigned Integer Addition of Two Operands with Carry Flag",
  "operands": [...],
  "architectures": {
    "SKL": {
      "measurement": {
        "TP_loop": "0.53",
        "TP_unrolled": "0.56",
        "TP_ports": "0.50",
        "uops": "1",
        "ports": "1*p06",
        "latencies": [
          {"start_op": "1", "target_op": "1", "cycles": "1"},
          ...
        ]
      }
    }
  }
}
```

## Common Microarchitectural Analysis Patterns

### Compare instruction alternatives across architectures

```bash
# Which is cheaper: POPCNT or manual bit-counting?
python3 .agents/skills/uops_info/uops_query.py search POPCNT --exact --arch SKL,ZEN4
```

### Check if SIMD is beneficial

```bash
# Compare scalar vs SSE2 vs AVX2 versions
python3 .agents/skills/uops_info/uops_query.py search PADDB --extension SSE2 --arch ICL
python3 .agents/skills/uops_info/uops_query.py search VPADDB --extension AVX2 --arch ICL
python3 .agents/skills/uops_info/uops_query.py search VPADDB --extension AVX512EVEX --arch ICL
```

### Analyze memory access patterns

```bash
# Check prefetch instruction costs
python3 .agents/skills/uops_info/uops_query.py search PREFETCH --arch SKL,ZEN4
```

## Notes

- Parse time is ~3-5 seconds due to the ~110 MB XML file size. This is expected.
- Data is from uops.info measurements (empirical), not vendor documentation.
- Instructions without performance data for a selected architecture will be omitted.
- The `--json` flag must come BEFORE the subcommand: `uops_query.py --json search ...`
