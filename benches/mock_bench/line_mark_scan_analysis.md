# Line Mark Scanning: Scalar vs Word-Level vs SSE2 Analysis

> **Target CPU**: AMD EPYC 7B13 (Zen 3 / znver3)
> **Tools**: llvm-mca 19.1.7, objdump, criterion benchmarks, uops.info
> **Date**: 2026-03-13

## 1. Context: The Line Mark Hot Loop

During every GC, Immix sweeps all allocated blocks by scanning a **line mark table** —
a contiguous 128-byte array where each byte records whether one 256-byte line is marked.
Two functions traverse this table on critical paths:

- **`Block::sweep()`** counts marked lines and holes (marked→unmarked transitions)
  to classify blocks as reusable, dead, or fully live.
- **`get_next_available_lines()`** searches for the next run of unmarked lines ("hole")
  that the allocator can bump-allocate into. This is on the **allocation fast path**.

Both scan the 128-byte table **one byte at a time** in the current MMTk code.

We tested **six strategies**, iteratively improving based on uarch analysis:

| # | Strategy | Idea |
|---|----------|------|
| 1 | **scalar** | Process 1 byte at a time: load, compare, branch |
| 2 | **word_u64** | Process 8 bytes as `u64`: XOR + byte extraction |
| 3 | **simd_sse2** | `pcmpeqb` + `pmovmskb` → per-bit hole scanning |
| 4 | **simd_sse2_v2** | `pcmpeqb` + `pslldq` + `pandn` + `psadbw` — stays in FP domain |
| 5 | **simd_sse2_popcnt** | `pcmpeqb` + `pmovmskb` + hardware `popcnt` (count-only) |
| 6 | **simd_sse2_sad** | `pcmpeqb` + `pand` + `psadbw` — no `pmovmskb` at all (count-only) |

---

## 2. Benchmark Results

### 2.1. Sweep Count (marked_lines + holes) — The Full Story

| Strategy | Time | vs Scalar | Key Insight |
|----------|------|-----------|-------------|
| scalar | 38.2 ns | 1.0× | LLVM auto-vectorizes with `shufps` + `andnps` |
| simd_sse2 (v1) | 51.1 ns | 0.75× ❌ | `pmovmskb` → per-bit scanning kills performance |
| word_u64 | 80.8 ns | 0.47× ❌ | Prevents auto-vectorization, byte extraction overhead |
| **simd_sse2_v2** | **8.3 ns** | **4.6× ✅** | `pslldq` + `pandn` + `psadbw`, stays in FP domain |

The **v2** strategy is a **4.6× improvement over scalar** and **6.2× over v1 SIMD**.
It applies the lesson from the uarch analysis: stay in the FP domain, use vectorial
transition detection, and use `psadbw` for horizontal byte accumulation.

### 2.2. Count Only (no hole detection)

| Strategy | Time | vs Scalar | Key Insight |
|----------|------|-----------|-------------|
| scalar | 15.6 ns | 1.0× | LLVM fully unrolls with `pcmpeqb` + `paddd` |
| simd_sse2 (v1) | 12.8 ns | 1.22× | `pmovmskb` + software popcount (~12 insns) |
| word_u64 | 34.1 ns | 0.46× ❌ | Prevents auto-vectorization |
| simd_sse2_popcnt | 3.78 ns | 4.1× ✅ | `pmovmskb` + hardware `popcnt` (1 insn) |
| **simd_sse2_sad** | **3.41 ns** | **4.6× ✅** | `pand` + `psadbw`, no `pmovmskb` at all |

The `psadbw` approach (**count_sad**) beats even hardware `popcnt` because it avoids
the 5-cycle `pmovmskb` FP→INT domain crossing entirely.

### 2.3. Hole Search

| Strategy | Alternating (64 holes) | Clustered (8 holes) |
|----------|------------------------|---------------------|
| scalar | 247 ns | 107 ns |
| word_u64 | 394 ns (1.6× slower) | 92 ns (1.17× faster) |
| simd_sse2 | 291 ns (1.2× slower) | **77 ns (1.40× faster)** |

SIMD chunk-skipping shines for clustered patterns where entire 16-byte runs
match one state. Alternating patterns (every byte differs) can't skip.

### 2.4. Sweep Count by Pattern

Pattern has no effect on scalar (auto-vectorized, branchless):

| Pattern | Scalar | SSE2 v1 |
|---------|--------|---------|
| alternating | 38.0 ns | 51.0 ns |
| clustered | 38.1 ns | 51.2 ns |
| dense (75%) | 38.2 ns | 51.2 ns |
| full | 38.3 ns | 51.0 ns |
| empty | 38.6 ns | 50.9 ns |

---

## 3. uops.info Instruction Performance Data (Zen 3)

All data from [uops.info](https://uops.info), measured on AMD Zen 3 hardware.

### 3.1. Instructions in the Auto-Vectorized Scalar Loop

| Instruction | µops | Latency | TP(unrl) | Ports | Role |
|---|---|---|---|---|---|
| `pcmpeqb xmm, xmm` | 1 | 1c | 0.25c | FP0123 | Compare 16 bytes |
| `punpcklbw xmm, xmm` | 1 | 1c | 0.50c | FP12 | Widen bytes→words |
| `punpcklwd xmm, xmm` | 1 | 1c | 0.50c | FP12 | Widen words→dwords |
| `shufps xmm, xmm, imm` | 1 | 1c | 0.50c | FP12 | Shift for transition detect |
| `andnps xmm, xmm` | 1 | 1c | 0.25c | FP0123 | Transition: `!curr & prev` |
| `paddd xmm, xmm` | 1 | 1c | 0.25c | FP0123 | Accumulate counts |

Sources:
[pcmpeqb](https://uops.info/html-instr/PCMPEQB_XMM_XMM.html) ·
[punpcklbw](https://uops.info/html-instr/PUNPCKLBW_XMM_XMM.html) ·
[shufps](https://uops.info/html-instr/SHUFPS_XMM_XMM_I8.html) ·
[andnps](https://uops.info/html-instr/ANDNPS_XMM_XMM.html) ·
[paddd](https://uops.info/html-instr/PADDD_XMM_XMM.html)

### 3.2. Key Bottleneck Instructions in SIMD v1

| Instruction | µops | Latency | TP(unrl) | Ports | Problem |
|---|---|---|---|---|---|
| **`pmovmskb r32, xmm`** | **1** | **5c** | **1.00c** | **FP45** | **FP→INT domain crossing** |
| Software `popcnt` (12 insns) | 12 | ~8c | ~3c | ALU | What `popcnt r32` does in 1c |
| `popcnt r32, r32` | 1 | 1c | 0.33c | ALU | 12× fewer µops than software |

Sources:
[pmovmskb](https://uops.info/html-instr/PMOVMSKB_R32_XMM.html) ·
[popcnt](https://uops.info/html-instr/POPCNT_R32_R32.html)

### 3.3. Instructions in the Improved SIMD v2

| Instruction | µops | Latency | TP(unrl) | Ports | Role |
|---|---|---|---|---|---|
| `pcmpeqb xmm, xmm` | 1 | 1c | 0.25c | FP0123 | Compare 16 bytes |
| `pslldq xmm, imm` | 1 | 1c | 0.50c | FP12 | Shift left by 1 byte for prev state |
| `psrldq xmm, imm` | 1 | 1c | 0.50c | FP12 | Extract last byte from prev chunk |
| `pandn xmm, xmm` | 1 | 1c | 0.25c | FP0123 | `!curr & prev` = hole transition |
| `pand xmm, xmm` | 1 | 1c | 0.25c | FP0123 | Map 0xFF → 0x01 |
| `psadbw xmm, xmm` | 1 | 3c | 1.00c | FP0123 | Sum 16 bytes horizontally |
| `paddd xmm, xmm` | 1 | 1c | 0.25c | FP0123 | Accumulate across iterations |

The v2 loop has **~12 instructions** per 16-byte chunk (8 iterations = ~96 total).
**No `pmovmskb`, no domain crossing, no per-bit scanning.**

---

## 4. llvm-mca Analysis

### 4.1. Summary (full 128-byte sweep)

```text
                        Instructions  RThroughput  IPC
Scalar (16 × 8B)          528 (33/iter)   6.5c/iter   4.09
SIMD v1 (8 × 16B)        ~960 (~120/iter)  ~16c/iter  ~4.0
SIMD v2 (8 × 16B)        ~96 (~12/iter)    ~1.5c/iter  ~4.0
```

### 4.2. Scalar Timeline (1 iteration = 8 bytes)

```text
[0,0]     DeeeeeeeeER         movd  (%rdi,%rax), %xmm8       ; load 4B
[0,1]     DeeeeeeeeER         movd  4(%rdi,%rax), %xmm6      ; load 4B
[0,2]     D========eER        pcmpeqb  %xmm3, %xmm8          ; compare
[0,5]     D==========eER      punpcklwd %xmm9, %xmm9         ; widen
[0,13]    D==========eER      shufps $3, %xmm9, %xmm7        ; shift prev
[0,14]    D===========eER     shufps $152, %xmm9, %xmm7      ; complete shift
[0,18]    D===========eER     andnps %xmm7, %xmm9            ; !curr & prev
[0,20]    D=============eER   psubd  %xmm10, %xmm0           ; accumulate holes
[0,22]    D=============eER   paddd  %xmm9, %xmm1            ; accumulate marks
```

**Critical path**: load (8c) → pcmpeqb (1c) → punpcklbw (1c) → punpcklwd (1c) →
shufps×2 (2c) → andnps (1c) → psubd (1c) = **15 cycles**.
OOO overlaps 2 independent chains → **6.5c/iter effective**.

Average wait: 8.6c scheduler, 0.4c ready, 2.9c retire.

### 4.3. SIMD v1 Timeline (1 iteration = 16 bytes, abbreviated)

```text
[0,0]     DeeeeeeeeER         movdqu (%rdi,%rsi), %xmm6      ; load 16B
[0,1]     D========eER        pcmpeqb %xmm0, %xmm6           ; compare
[0,2]     D=========eER       pmovmskb %xmm6, %r9d           ; 5c LAT → INT!
[0,9]     D=========eE---R    testb $1, %r9b                 ; wait for mask
[0,10]    D==========eE--R    cmovne %ecx, %r8d              ; bit 0
...                                                            ; 14 more bits
[0,31]    D=========eE-R      shrl %r8d                      ; popcount start
[0,47]    D===================eER  addl %r8d, %eax            ; final accumulate
```

Average wait: 8.6c scheduler, 0.2c ready, **4.7c retire** (62% higher than scalar).

### 4.4. Port Pressure

#### Scalar (per iteration)
```text
FP0123: 10 µops   FP12: 9 µops   FP45: 0   ALU: 3 µops
```
Excellent FP balance. Shuffles on FP12, arithmetic on FP0123.

#### SIMD v1 (per iteration)
```text
FP0123: 2 µops   FP45: 1 µops   ALU: ~90 µops
```
Almost all work dumped into integer ALU after `pmovmskb`. FP underutilized.

#### SIMD v2 (per iteration, estimated)
```text
FP0123: ~8 µops   FP12: ~3 µops   FP45: 0   ALU: ~1 µop
```
All work stays in FP. `psadbw` (FP0123) is the throughput bottleneck at 1.00c.

---

## 5. Evolution of Strategies — What We Learned

### Attempt 1: word_u64 — Preventing Auto-Vectorization (0.47×)

**Idea**: Load 8 bytes as `u64`, XOR with broadcast target, extract bytes.

**What went wrong**: By manually loading as `u64` and calling `to_ne_bytes()`,
we prevented the compiler from recognizing the simple byte-comparison pattern.
LLVM treats explicit byte extraction as intentional and won't re-vectorize.
Result: 2× slower than letting the compiler auto-vectorize.

**Lesson**: For simple contiguous byte-array scans, write the simplest possible
scalar loop and trust LLVM's auto-vectorizer.

### Attempt 2: simd_sse2 v1 — The `pmovmskb` Trap (0.75×)

**Idea**: Use `pcmpeqb` to compare 16 bytes at once, `pmovmskb` to extract a bitmask,
then scan bits for hole counting + popcount for mark counting.

**What went wrong**:
1. `pmovmskb` has **5-cycle latency** (FP→INT domain crossing on Zen 3)
2. Per-bit hole scanning: 16 × `test`/`cmov` = ~32 instructions, all serial
3. Software popcount: ~12 instructions for what hardware does in 1
4. Total: ~120 insns/iter vs compiler's ~33 insns/iter

**Lesson**: `pmovmskb` collapses SIMD data into a scalar register. If you then
process bits one-at-a-time, you've thrown away the SIMD advantage entirely.

### Attempt 3: word_u64 with SWAR zero-byte trick — Correctness Bug

**Idea**: Use Hacker's Delight `(x - 0x01...01) & ~x & 0x80...80` to detect
zero bytes after XOR.

**What went wrong**: Borrow propagation across byte boundaries. When adjacent
bytes after XOR are `[0x00, 0x01]`, the subtraction `0x01 - 0x01 = 0x00`
doesn't borrow, but `0x00 - 0x01 = 0xFF` borrows from the next byte, corrupting
the zero-detection. Result: 128 marks reported instead of 64.

**Lesson**: SWAR byte tricks are fragile. The zero-byte detection formula has
known false positives for values near 0x00/0x80. Prefer SIMD `pcmpeqb` which
is correct by construction.

### Attempt 4: simd_sse2_v2 — Vectorial Transitions (4.6× ✅)

**Idea**: Mirror the compiler's auto-vectorization strategy but do it explicitly
on 16 bytes at a time:
1. `pcmpeqb` → byte comparison (0xFF or 0x00)
2. `pslldq` shift left by 1 byte to get "previous" state
3. `psrldq` + `por` to carry last byte from previous chunk
4. `pandn` = `!current & previous` → hole transitions
5. `pand(ones)` + `psadbw` → horizontal byte sum (no `pmovmskb`!)

**Why it works**: Everything stays in the FP domain with 1-cycle latencies.
`psadbw` sums 16 bytes in 1 instruction (3c latency but 1c throughput).
No FP→INT crossing, no per-bit scanning, no software popcount.

### Attempt 5: simd_sse2_popcnt — Hardware Popcount (4.1× ✅)

**Idea**: Same as v1 but enable hardware `popcnt` with `#[target_feature]`.

**Result**: 3.78 ns vs v1's 12.8 ns. Hardware popcnt replaces 12 instructions
with 1, eliminating the software popcount bottleneck. Still slower than `count_sad`
because `pmovmskb` still costs 5 cycles.

### Attempt 6: simd_sse2_sad — psadbw Without pmovmskb (4.6× ✅)

**Idea**: `pcmpeqb` → `pand(0x01)` → `psadbw` → `paddd`. Never leave FP domain.

**Result**: 3.41 ns — the fastest count-only strategy. Proof that avoiding
`pmovmskb` entirely is better than using it + hardware `popcnt`.

---

## 6. The Key Insight: Stay in the FP Domain

```
❌ Slow path (v1):     pcmpeqb → pmovmskb → [INT: per-bit scan + popcnt]
                        FP (2c)   FP→INT(5c)   INT (~20c serial)

✅ Fast path (v2/sad): pcmpeqb → pand/pslldq/pandn → psadbw → paddd
                        FP (1c)   FP (1c each)         FP(3c)   FP(1c)
```

On Zen 3, the FP domain has:
- **4 execution ports** (FP0123) for arithmetic/logic
- **2 execution ports** (FP12) for shuffles
- **1-cycle latency** for almost everything
- **No domain-crossing penalty** between operations

The INT domain is efficient for its own operations, but the 5-cycle `pmovmskb`
penalty to get data from FP→INT is devastating for a tight loop.

---

## 7. Practical Implications for MMTk

### What to optimize (with expected speedup)

| Target | Strategy | Speedup |
|--------|----------|---------|
| `Block::sweep()` | Replace scalar loop with `simd_sse2_v2` approach | **4.6×** |
| Separate `marked_lines` counting | `psadbw`-based counting | **4.6×** |
| `get_next_available_lines()` | SSE2 chunk-skipping | **1.4× clustered** |

### What NOT to optimize

| Target | Reason |
|--------|--------|
| Word-level (u64) SWAR tricks | Prevents auto-vectorization, fragile carry bugs |
| `pmovmskb` → per-bit scanning | Collapses SIMD → scalar, loses all benefit |

---

## 8. Correctness Verification

All strategies are verified against the scalar baseline across:
- **5 standard patterns**: alternating, clustered, dense, full, empty
- **12 single-mark positions**: 0, 1, 7, 8, 15, 16, 17, 31, 32, 63, 64, 127
- **Chunk boundary**: marks at bytes 14-17 (tests `pslldq` carry-over)
- **Chunk-aligned transitions**: first chunk only, alternating chunks
- **All mark state values**: 1 through `LINE_MAX_MARK_STATE`
- **Pseudo-random pattern**: deterministic PRNG with seed 0xDEADBEEF

---

## 9. References

- [uops.info — PCMPEQB XMM (Zen 3)](https://uops.info/html-instr/PCMPEQB_XMM_XMM.html): 1 µop, 0.25c throughput, 1c latency, FP0123
- [uops.info — PMOVMSKB R32,XMM (Zen 3)](https://uops.info/html-instr/PMOVMSKB_R32_XMM.html): 1 µop, 1.00c throughput, **5c latency**, FP45
- [uops.info — PSADBW XMM (Zen 3)](https://uops.info/html-instr/PSADBW_XMM_XMM.html): 1 µop, 1.00c throughput, 3c latency, FP0123
- [uops.info — PSLLDQ XMM (Zen 3)](https://uops.info/html-instr/PSLLDQ_XMM_I8.html): 1 µop, 0.50c throughput, 1c latency, FP12
- [uops.info — POPCNT R32,R32 (Zen 3)](https://uops.info/html-instr/POPCNT_R32_R32.html): 1 µop, 0.33c throughput, 1c latency
- llvm-mca 19.1.7 (`-mcpu=znver3`): assembly extracted via `objdump -d`
