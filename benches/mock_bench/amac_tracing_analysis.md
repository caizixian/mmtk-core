# Microarchitectural Optimization of AMAC for GC Tracing

> **Target CPU**: AMD EPYC 7B13 (Zen 3 / znver3), dual-socket
> **Heap**: 4M objects × 256 bytes = 1 GB (32× per-CCD L3)
> **Date**: 2026-03-13
> **Tools**: `objdump`, `llvm-mca -mcpu=znver3`, `uops_query.py` (uops.info Zen 3 data)

## 1. Problem Statement

AMAC (Asynchronous Memory Access Chaining) promises optimal MLP by
maintaining N independent pipeline states, but empirically is **15-68%
slower than baseline** GC tracing.  Why?  And can we fix it?

---

## 2. Root Cause Analysis: Dispatch Overhead

### 2.1. Actual Compiled Assembly (from `objdump`)

The AMAC dispatch back-edge runs on **every state transition** — between
every case arm.  Extracted from `trace_bfs_amac<32>` at `0x32abd0`:

```asm
; ── AMAC dispatch (runs EVERY state visit) ────────────────────
  32abd0:  cmp     rbp, 0x20                          ; done_count >= 32?
  32abda:  inc     r14d                               ; cursor++
  32abdd:  and     r14d, 0x1f                         ; cursor %= 32
  32abe1:  lea     rax, [r14+r14*2]                   ; cursor * 3 (24B stride)
  32abe5:  lea     rbx, [rsp+rax*8]                   ; &states[cursor]
  32abe9:  add     rbx, 0x88                          ; stack offset
  32abf0:  movzx   eax, BYTE PTR [rsp+rax*8+0x98]    ; load stage byte
  32abf8:  movsxd  rax, DWORD PTR [r12+rax*4]        ; jump table entry
  32abfc:  add     rax, r12                           ; compute target
  32abff:  jmp     rax                                ; INDIRECT BRANCH
```

Each case arm is reached via this dispatch, then falls back into it.
For example, the ObjPrefetched → mark check arm at `0x32ac40`:

```asm
; ── Case ObjPrefetched: check mark bit ───────────────────────
  32ac40:  mov     rax, QWORD PTR [rbx]               ; load state.obj
  32ac43:  mov     rcx, QWORD PTR [rax]               ; load header
  32ac46:  test    cl, 0x1                             ; check mark bit
  32ac49:  jne     32ac60                              ; skip if marked
  32ac4b:  or      rcx, 0x1                            ; set mark bit
  32ac4f:  mov     QWORD PTR [rax], rcx               ; write back
  32ac52:  mov     BYTE PTR [rbx+0x10], 0x3           ; stage = 3
  32ac56:  jmp     32abd0                              ; → dispatch
; ── Already marked path ──────────────────────────────────────
  32ac60:  mov     BYTE PTR [rbx+0x10], 0x0           ; stage = Empty
  32ac64:  jmp     32abd0                              ; → dispatch
```

### 2.2. llvm-mca Analysis (znver3)

| Code Fragment | Instructions | µops | RThroughput | IPC | Notes |
|---|---|---|---|---|---|
| **Dispatch only** | 10 | 12 | 2.0 | 4.33 | Runs between ALL case arms |
| **Empty+dispatch** | 15 | 17 | 2.8 | 4.53 | 5 useful + 10 dispatch |
| **Mark+dispatch** | 16 | 18 | 3.0 | 5.02 | 6 useful + 10 dispatch |
| **Interleaved loop** | 20 | 20 | 3.7 | 5.12 | **All 20 are useful** |

**Key ratio: In AMAC, dispatch consumes 12/17–12/18 = 67–71% of µops per case arm.**
Only ~1/3 of the executed µops do useful work.

**llvm-mca timeline — Mark case + dispatch (znver3, 3 iterations):**

The `D` column = dispatch, `e` = execute, `E` = complete, `R` = retire.
Iteration `[0]` shows the ObjPrefetched case (6 useful insn) flowing into
dispatch (10 insn).  Note how `movzx→movsxd→add→jmp` serialize at the end:

```
[0,0]     DeeeeeER                  mov rax, [rbx]           ; load state.obj
[0,1]     D=====eeeeeER             mov rcx, [rax]           ; load header
[0,2]     D==========eER            test cl, 0x1             ; mark bit?
[0,3]     D==========eER            or  rcx, 0x1             ; set mark
[0,4]     D===========eER           mov [rax], rcx           ; write back
[0,5]     D===========eER           mov BYTE [rbx+0x10], 0x3 ; stage = Ready
          ─── dispatch overhead starts here ───────────────────────────
[0,6]     .DeE----------R           cmp rbp, 0x20            ; done_count?
[0,7]     .DeE----------R           inc r14d                 ; cursor++
[0,8]     .D=eE---------R           and r14d, 0x1f           ; mod 32
[0,9]     .D==eeE-------R           lea rax, [r14+r14*2]     ; ×3 (2µops)
[0,10]    .D===eeE------R           lea rbx, [rsp+rax*8]     ; (2µops)
[0,11]    .D=====eE-----R           add rbx, 0x88
[0,12]    .D===eeeeeE---R           movzx eax, BYTE [...]    ; 5c load
[0,13]    .D========eeeeeER         movsxd rax, DWORD [...]  ; 5c dep load!
[0,14]    .D=============eER        add rax, r12
[0,15]    .D==============eER       jmp rax                  ; INDIRECT
```

The useful work (lines 0-5) takes ~12 cycles, then the dispatch chain
(lines 6-15) takes another ~15 cycles — the `movzx→movsxd` dependent
load chain alone costs ~10 cycles serialized, visible as `eeeee` stalls.

### 2.3. Per-Instruction Cost from uops.info (Zen 3)

| Instruction (from dispatch) | µops | Latency | TP(unrl) | Role |
|---|---|---|---|---|
| `cmp rbp, 0x20` | 1 | 1c | 0.25 | done check |
| `inc r14d` | 1 | 1c | 0.25 | cursor++ |
| `and r14d, 0x1f` | 1 | 1c | 0.25 | modulo N |
| `lea rax,[r14+r14*2]` | **2** | **2c** | 0.25 | 3-component LEA → 2µops on Zen 3 |
| `lea rbx,[rsp+rax*8]` | **2** | **2c** | 0.25 | 3-component LEA → 2µops on Zen 3 |
| `add rbx, 0x88` | 1 | 1c | 0.25 | stack offset |
| `movzx eax, BYTE [...]` | 1 | **4-5c** | 0.33 | load stage byte (L1 hit) |
| `movsxd rax, DWORD [...]` | 1 | **5-6c** | 0.33 | jump table load (dep on movzx!) |
| `add rax, r12` | 1 | 1c | 0.25 | compute target |
| `jmp rax` | 1 | **~15c mispred** | 2.75 | **INDIRECT BRANCH** |
| **Total** | **12** | | **2.0** | |

**Critical dependency chain:**

```
inc → and → lea → lea → movzx(5c) → movsxd(5c) → add → jmp(mispred 15c)
  1c    1c    2c    2c     5c dep       5c dep      1c     ≥15c mispred
                                                     ═══════════════
                                                     Total: ~32 cycles
```

The `movzx → movsxd` is a **dependent load chain**: the stage byte determines
the jump table index, creating a serialized 10-cycle load pipeline *before*
the indirect branch even executes.

**llvm-mca timeline — dispatch only (znver3, 3 iterations):**

```
[0,0]     DeER                       cmp  rbp, 32
[0,1]     DeER                       inc  r14d
[0,2]     D=eER                      and  r14d, 31
[0,3]     D==eeER                    lea  rax, [r14+r14*2]    ; 2µops
[0,4]     .D===eeER                  lea  rbx, [rsp+rax*8]    ; 2µops
[0,5]     .D=====eER                 add  rbx, 136
[0,6]     .D===eeeeeER               movzx eax, [rsp+...+152] ; 5c dep!
[0,7]     .D========eeeeeER          movsxd rax, [r12+rax*4]  ; 5c dep!
[0,8]     .D=============eER         add  rax, r12
[0,9]     . D=============eER        jmp  rax
```

Key observations:
- `cmp`, `inc` dispatch immediately (cycle 0) — no dependencies
- `and` waits 1c for `inc` result
- Both `lea` instructions take 2c each (3-component LEA = 2µops)
- `movzx` starts early (cycle 3) but takes 5c for load
- `movsxd` can't start until `movzx` completes → **serialized 10c load chain**
- Total dispatch latency: **15 cycles** (visible as `[0,9]` retiring at cycle 16)

### 2.4. Indirect Branch Misprediction

With 5 match arms (Empty, SlotPrefetched, ObjPrefetched, ReadyToProcess,
Drained), the branch target alternates unpredictably:

- Objects transition `Empty→Slot→Obj→Ready→Empty` (4 different targets)
- Already-marked objects short-circuit `Obj→Empty` (different target)
- The 32 slots are visited round-robin, each at a different stage

Zen 3's indirect branch predictor (ITTAGE) handles simple patterns well,
but this ~5-way alternation across 32 interleaved states exceeds the
pattern history — estimated **60-80% misprediction rate** = ~15 cycle
penalty per dispatch on average.

### 2.5. Additional Overheads

| Overhead | Cost | Per Object (3-4 visits) |
|---|---|---|
| Dispatch µops | 12 µops × 3.5 | **42 µops** |
| Misprediction | ~15c × 3.5 | **~52 cycles** |
| State from stack | L1 loads for stage + slot + obj | 3-4 loads |
| Stack frame | `sub rsp, 0x388` (904 bytes!) | Register pressure |

AMAC-32 places 32 × 24-byte states = 768 bytes on the stack, consuming
~12 cache lines.  While these stay in L1, they compete with the OOO
engine's store buffer and load queue.

---

## 3. Case Arms: Useful Work Analysis

| Case Arm | Useful Instructions | Per-Case µops | With Dispatch |
|---|---|---|---|
| Empty→Refill | 5 (load slot, store state, prefetch, stage write, cursor++) | 5 | **17** |
| SlotPrefetched→Load | 5 (load, deref, null-check, store, prefetch) | 5 | **17** |
| ObjPrefetched→Mark | 6 (load, deref, test, or, store, stage write) | 6 | **18** |
| ReadyToProcess→Scan | ~20 (scan N_REFS fields) | ~20 | **32** |
| Drained→Skip | 0 (just dispatch!) | 0 | **12** |

The **Drained** case is pure overhead — 12 µops to do nothing.  With
`32 - N_active` slots drained, every round visits several drained slots
wastefully.

---

## 4. Optimized Variants

### 4.1. Interleaved Pipeline (Strategy 6)

The interleaved loop at `0x32daf0` (D=16) compiles to:

```asm
; ── Interleaved hot loop (one iteration = one slot) ──────────
loop:
  32daf0:  lea     rax, [rcx+r13]           ; bounds check
  32daf4:  inc     rax
  32daf7:  inc     r13                      ; i++
  32dafa:  cmp     rax, 0x20               ; end of packet?
; Stage 0: prefetch edge at i+2D
  32db04:  lea     rax, [r13+0x20]          ; i+32  (2D=32)
  32db08:  cmp     rax, rbx                ; bounds check
  32db0d:  mov     rax, [rdx+r13*8+0x100]  ; load packet[i+2D]
  32db15:  prefetchnta BYTE PTR [rax]      ; ← PREFETCH edge
; Stage 1: load slot at i+D, prefetch object
  32db18:  cmp     r13, rbx                ; bounds check
  32db1d:  mov     rax, [rdx+r13*8]        ; load packet[i+D]
  32db21:  mov     rax, [rax]              ; deref → objref
  32db24:  test    rax, rax                ; null check
  32db29:  prefetchnta BYTE PTR [rax]      ; ← PREFETCH object
; Stage 2: process current slot
  32db2c:  mov     rax, [rdx+r13*8-0x100]  ; packet[i]
  32db34:  mov     r15, [rax]              ; deref → objref
  32db37:  test    r15, r15                ; null check
  32db3c:  mov     rax, [r15]              ; load header
  32db3f:  test    al, 0x1                 ; mark bit?
  32db43:  or      rax, 0x1               ; set mark
  32db47:  mov     [r15], rax             ; write back
; ... scan fields ...
  jmp     loop
```

**llvm-mca (znver3):**

```
Instructions: 20    µops: 20    RThroughput: 3.7    IPC: 5.12
```

**100% of µops are useful work.** No dispatch overhead.  The loop
back-edge (`jmp loop`) is perfectly predicted (>99.9%) since it's a
simple loop-closing branch.

**llvm-mca timeline — interleaved loop (znver3, 3 iterations):**

All three pipeline stages execute **in parallel** via ILP.  The OOO
engine overlaps Stage 0 prefetches, Stage 1 loads, and Stage 2 processing:

```
[0,0]     DeER                       lea  rax,[rcx+r13]       ; bounds
[0,1]     D=eER                      inc  rax
[0,2]     DeE-R                      inc  r13                 ; i++
[0,3]     D==eER                     cmp  rax, 32
──── Stage 0: prefetch edge at i+2D ─────────────────────────────
[0,4]     D=eE-R                     lea  rax,[r13+32]
[0,5]     D==eER                     cmp  rax, rbx
[0,6]     .DeeeeeER                  mov  rax,[rdx+r13*8+256] ; 5c load
[0,7]     .D=====eeeeeER             prefetchnta [rax]        ; fire&forget
──── Stage 1: load slot i+D, prefetch obj ───────────────────────
[0,8]     .DeE---------R             cmp  r13, rbx
[0,9]     .DeeeeeE-----R             mov  rax,[rdx+r13*8]     ; 5c load
[0,10]    .D=====eeeeeER             mov  rax,[rax]           ; deref obj
[0,11]    .D==========eER            test rax, rax
[0,12]    .D==========eeeeeER        prefetchnta [rax]
──── Stage 2: process current slot ──────────────────────────────
[0,13]    .DeeeeeE----------R        mov  rax,[rdx+r13*8-256] ; 5c load
[0,14]    .D=====eeeeeE-----R        mov  r15,[rax]           ; deref obj
[0,15]    .D==========eE----R        test r15, r15
[0,16]    .D==========eeeeeER        mov  rax,[r15]           ; header
[0,17]    .D===============eER       test al, 1               ; mark?
[0,18]    .D===============eER       or   rax, 1              ; set mark
[0,19]    .D================eER      mov  [r15], rax          ; write back
```

Compare with the AMAC dispatch timeline: here **all 20 instructions are useful**.
Stage 0/1/2 loads overlap naturally — no dispatch, no indirect branch, no
state machine overhead.  Iteration throughput: ~4 cycles (vs ~15c for AMAC dispatch alone).

### 4.2 Staged-Batch Pipeline (Strategy 5)

```rust
// Pass 1: load N slot contents → N objrefs, issue N prefetches
for &slot in batch { prefetch_nta(load(slot).header()); }

// Pass 2: check N marks, filter to newly-marked
for &obj in &objrefs { if !marked(obj) { mark(obj); } }

// Pass 3: scan fields
for &obj in &to_scan { scan(obj, &mut out); }
```

Each pass compiles to a tight loop (3-insn back-edge: `inc; cmp; jb`).
MLP comes from batching N accesses before consuming results.

---

## 5. Benchmark Results

### 5.1. Header-Based Marks

| Strategy | Time (ms) | vs Baseline | vs Prefetch | Useful µops % |
|----------|-----------|-------------|-------------|---------------|
| Baseline | 512 | — | — | ~100% |
| **Prefetch E32/O16** | **361** | **-29.5%** | — | ~95% |
| AMAC-4 | 854 | +66.8% | +136% | ~33% |
| AMAC-8 | 726 | +41.8% | +101% | ~33% |
| AMAC-16 | 628 | +22.7% | +73.9% | ~33% |
| AMAC-32 | 619 | +20.9% | +71.5% | ~33% |
| Staged-Batch 8 | 530 | +3.5% | +46.8% | ~100% |
| Staged-Batch 16 | 443 | **-13.5%** | +22.7% | ~100% |
| **Staged-Batch 32** | **381** | **-25.6%** | +5.5% | ~100% |
| Interleaved D=8 | 382 | -25.4% | +5.8% | ~100% |
| **Interleaved D=16** | **358** | **-30.1%** | **-0.8%** | ~100% |
| Interleaved D=32 | 370 | -27.7% | +2.5% | ~100% |

### 5.2. Side-Metadata Marks (Focused Run)

| Strategy | Time (ms) | vs SM Baseline | vs 3-Target PF | Notes |
|----------|-----------|----------------|-----------------|-------|
| SM Baseline | 850 | — | — | Extra cache miss for meta |
| **3-Target Prefetch** | **523** | **-38.5%** | — | edge+obj+meta |
| **SM Staged-Batch 16** | **491** | **-42.2%** | **-6.1%** | Dedicated meta pass |
| **SM Staged-Batch 32** | **490** | **-42.4%** | **-6.3%** | |
| SM Interleaved D=16 | 560 | -34.1% | +7.1% | Less meta PF time |
| SM Interleaved D=32 | 578 | -32.0% | +10.5% | |

---

## 6. Why Staged-Batch Wins for Side-Metadata

For header marks, the interleaved loop slightly beats staged-batch
(**358 vs 381 ms**) because a single flat loop has less overhead than
managing batch boundaries (clearing/filling `Vec` scratch buffers).

For side-metadata, the staged-batch loop **dramatically beats**
interleaved (**491 vs 560 ms**).  The reason is the **dedicated
metadata prefetch pass**:

```
Staged-batch:     Interleaved:
  Pass 1: load slots, prefetch obj    Stage 0: prefetch edge (i+2D)
  Pass 2: prefetch meta  ← ALL N     Stage 1: load slot, prefetch obj+meta
  Pass 3: check marks                 Stage 2: process (i)
  Pass 4: scan
```

In staged-batch, Pass 2 issues ALL N metadata prefetches at once,
giving them the entire duration of Pass 3 to complete (~50-100 cycles
per prefetch at L3/DRAM latency).

In interleaved, the metadata prefetch at `i+D` has only D iterations
to complete before it's consumed at `i`.  With D=16 and ~4-5c per
iteration, that's ~70-80 cycles — which may not be enough for a DRAM
access (~100-150 ns = ~300-450 cycles at 3 GHz).

---

## 7. Quantitative Overhead Accounting

Using the benchmark data and µarch analysis, we can estimate the
AMAC dispatch overhead:

```
AMAC-32 (header):               619 ms
Staged-Batch-32 (header):       381 ms
Difference (pure overhead):     238 ms

Objects traced per iteration:   4,110,753
States visited per object:      ~3.5 (Empty→Slot→Obj→Ready, some skip)
Total dispatches:               ~14.4M
Overhead per dispatch:          238ms / 14.4M ≈ 16.5 ns ≈ 50 cycles

Expected from µarch analysis:
  - RThroughput:                2.0 cycles (12 µops at dispatch width 6)
  - Dep chain (movzx→movsxd):  ~10 cycles  
  - Misprediction:             ~15 cycles × ~70% = ~10.5 cycles
  - Total:                     ~22-25 cycles

Measured overhead:              ~50 cycles (remaining ~25c from:
                                secondary effects, OOO window limits,
                                L1 pressure from 768B state array,
                                frontend fetch/decode of case arm code)
```

---

## 8. Comprehensive uops.info Reference (Zen 3)

| Instruction | Form | µops | Latency | TP(unrl) | Notes |
|---|---|---|---|---|---|
| `JMP` | R64 (indirect) | 1 | — | 2.75 | Misprediction ≈15c |
| `JMP` | M64 (mem-indirect) | 1 | — | 1.42 | |
| `MOVZX` | R32, M8 | 1 | addr=4c mem=≤5c | 0.33 | Stage byte load |
| `MOVSXD` | R64, M32 | 1 | addr=5c mem=≤6c | 0.33 | Jump table entry |
| `MOVSXD` | R64, R32 | 1 | 1c | 0.25 | |
| `PREFETCHNTA` | M512 | 1 | — | 0.33 | 3/cycle throughput |
| `LEA` | 2-component (R+R) | 1 | 1c | 0.33 | e.g. `[rax+rbx]` |
| `LEA` | 3-component (R+R*S) | **2** | **2c** | 0.25 | e.g. `[r14+r14*2]` |
| `TEST` | R8, I8 | 1 | 1c | 0.25 | Mark bit check |
| `OR` | R64, I8 | 1 | 1c | 0.25 | Set mark bit |
| `CMP` | R64, I32 | 1 | 1c | 0.25 | |
| `INC` | R32 | 1 | 1c | 0.25 | |
| `AND` | R32, I32 | 1 | 1c | 0.25 | Modulo N |
| `MOV` | R64, M64 | 1 | ≤5c | 0.33 | Object header load |
| `MOV` | M64, R64 | 1 | — | 1.00 | Store (1 store port) |

**Key Zen 3 observations:**
- 3-component LEA (`[base + index*scale]`) costs **2µops/2c**, not 1µop.
  AMAC dispatch has TWO of these (lines 5-6), adding 2 extra µops.
- `PREFETCHNTA` has throughput of 3/cycle — prefetching is nearly free
  in terms of port pressure.
- Indirect `JMP R64` has TP=2.75, but the real cost is misprediction.
- Store throughput is 1/cycle (single store port), which can bottleneck
  scan loops with many field writes.

---

## 9. Recommendations for MMTk

1. **For header-marks**: Use **Interleaved D=16** — 358ms, simplest code,
   matches best-known prefetching.

2. **For side-metadata marks** (standard MMTk): Use **Staged-Batch N=16-32**
   — 491ms, the dedicated metadata prefetch pass gives an extra 6% over
   three-target prefetching.

3. **Never use AMAC's round-robin `match` dispatch** in Rust on Zen 3.
   The indirect jump table dispatch wastes 67% of µops.

4. **Performance gap**: The remaining gap between staged-batch-32 (381ms)
   and prefetch (361ms) for header marks comes from `Vec` management
   overhead (clearing/filling scratch buffers).  Further optimization:
   use fixed-size arrays instead of `Vec`, or eliminate the filtering
   pass entirely.

---

## 10. Assembly Files for Reproduction

| File | Contents |
|---|---|
| `asm_amac32.s` | Full `trace_bfs_amac<32>` disassembly (719 lines) |
| `asm_interleaved8.s` | Full `trace_bfs_interleaved<8>` disassembly |
| `asm_interleaved16.s` | Full `trace_bfs_interleaved<16>` disassembly |
| `asm_staged8.s` | Full `trace_bfs_staged_batch<8>` disassembly |
| `asm_prefetch.s` | Full `trace_bfs_prefetch_e32_o16` disassembly |
| `mca_amac_dispatch.s` | AMAC dispatch fragment for llvm-mca |
| `mca_amac_case_empty.s` | AMAC Empty case + dispatch for llvm-mca |
| `mca_amac_case_mark.s` | AMAC ObjPrefetched case + dispatch for llvm-mca |
| `mca_interleaved.s` | Interleaved hot loop for llvm-mca |

Run: `llvm-mca -mcpu=znver3 -iterations=100 <file>.s`
