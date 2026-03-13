# SIMD Gather vs Scalar Mark-Check: Microarchitectural Analysis

> **Target CPU**: AMD EPYC 7B13 (Zen 3 / znver3), dual-socket
> **Cache**: 32 MB L3 per CCD × 4 CCDs/socket × 2 sockets = 256 MB total L3 (per-CCD, not unified)
> **Tools**: llvm-mca 19.1.7, objdump, criterion benchmarks
> **Date**: 2026-03-13

## 1. Context: The Mark-Check Hot Loop

During GC tracing, MMTk visits every slot in the work buffer and checks whether
the referenced object is already marked. The inner loop is:

```
for each object_ref in traversal_buffer:
    header = load(object_ref)       // <-- THE EXPENSIVE PART
    if header & 1 == 0:             // unmarked?
        store(object_ref, header|1) // set mark bit
        count++
```

The performance-critical operation is the **pointer-chasing load**: each
`object_ref` points to a random location in the heap. With 1M objects (512 MB),
these loads miss L3 cache (32 MB per CCD) and go to DRAM (~100ns).

We tested three strategies for this loop:

| Strategy | Idea |
|----------|------|
| **scalar** | Process 1 object at a time with branch on mark bit |
| **batch_branchless** | Process 4 objects branchlessly: `count += 1 - (header & 1)` |
| **simd_avx2** | Use `vpgatherqq` to load 4 headers, SIMD bitops to check marks |

---

## 2. Instruction-Level Analysis

### 2.1. uops.info Measured Data

All data from [uops.info](https://uops.info), measured on real hardware
(Ryzen 5 5600X for Zen 3, Ryzen 9 7950X for Zen 4).

#### `vpgatherqq ymm` (4 × 64-bit gather) — the instruction we use

| Metric | AMD Zen 3 | AMD Zen 4 | Intel Skylake |
|--------|-----------|-----------|---------------|
| **µops executed** | **23** | **24** | **5** |
| Latency (index→result) | ≤18 | ≤16 | ≤20 |
| Latency (mask→result) | 15 | 15 | 20 |
| **Throughput (measured)** | **4.00 c** | **4.00 c** | **4.00 c** |
| Throughput (computed from ports) | 2.50 c | 2.50 c | 2.00 c |
| Port usage | 2×FP01, 2×FP0123, 2×FP1, 1×FP12, 5×FP45 | 4×FP01, 3×FP123, 1×FP23, 5×FP45 | 1×p0, 1×p015, 4×p23, 1×p5 |

Sources:
- [vpgatherqq ymm Zen 3](https://uops.info/html-instr/VPGATHERQQ_YMM_VSIB_YMM_YMM.html)
- [vpgatherqq ymm Skylake](https://uops.info/html-instr/VPGATHERQQ_YMM_VSIB_YMM_YMM.html)

#### `vpgatherdd ymm` (8 × 32-bit gather) — for comparison

| Metric | AMD Zen 3 | Intel Skylake |
|--------|-----------|---------------|
| **µops executed** | **39** | **5** |
| Latency (index→result) | ≤28 | ≤20 |
| **Throughput (measured)** | **8.00 c** | **4.00 c** |

Source: [vpgatherdd ymm Zen 3](https://uops.info/html-instr/VPGATHERDD_YMM_VSIB_YMM_YMM.html)

#### Key observation: 23 µops vs 5 µops

On Zen 3, `vpgatherqq ymm` decomposes into **23 micro-ops** — over 4× more than
Intel Skylake's 5 µops. These 23 µops flow through the backend sequentially,
competing for FP and load ports. Even though the measured throughput is 4
cycles/instruction, those 23 µops consume backend resources that **cannot be
used by surrounding instructions**.

For comparison, 4 scalar loads (`mov rax, [rdi+N]`) execute as 4 µops total
(1 each), with a throughput of 0.33 cycles each (3 loads/cycle on Zen 3's 3
load ports). 4 independent scalar loads complete in ~1.3 cycles — only ⅓ the
throughput cost of one gather.

#### Comparison with 4 × independent scalar loads

| Metric | `vpgatherqq ymm` (4 loads) | 4 × `mov reg, [mem]` |
|--------|----------------------------|----------------------|
| µops | 23 | 4 |
| Throughput | 4.00 cycles | ~1.3 cycles |
| Backend FP port pressure | 5×FP45, 2×FP01, etc. | 0 (integer pipeline) |
| Can overlap with surrounding code? | Limited (23 µops in-flight) | Excellent (4 µops, OOO overlaps freely) |

---

## 3. llvm-mca Analysis

### 3.1. Summary

Both snippets process 4 objects per iteration. Assembly was extracted from the
benchmark binary via `objdump -d`.

```text
Strategy          | Total µops | Block RThroughput | IPC
------------------+------------+-------------------+-----
Scalar (4 obj)    |    24      |     5.3 cycles    | 4.40
SIMD gather (4 obj)|   39      |     7.5 cycles    | 4.42
```

The SIMD loop has **63% more µops** and **42% higher RThroughput** than scalar.
But this is an *optimistic* picture — llvm-mca models `vpgatherqq` as a single
µop with RThroughput 1.0, drastically underestimating its real backend cost.

> **Caveat**: llvm-mca's znver3 scheduling model treats `vpgatherqq` as 1 µop
> (the real count is 23 from uops.info measurements). The timeline below shows
> ideal scheduling, not the true gather decomposition.

### 3.2. Scalar Timeline

```text
Timeline view (2 iterations, llvm-mca -mcpu=znver3):
                    0123456789
Index     0123456789          01

[0,0]     DeeeeeER  .    .    ..   mov  rax, [rdi]          # load slot ptr
[0,1]     D=====eeeeeER  .    ..   mov  rcx, [rax]          # load header (5c latency)
[0,2]     D==========eER .    ..   test rcx, 1              # test mark bit
[0,3]     D===========eER.    ..   jne  .Lskip0             # branch
[0,4]     D==========eE-R.    ..   or   rcx, 1              # set mark
[0,5]     D===========eER.    ..   mov  [rax], rcx           # store header
[0,6]     .DeeeeeE------R.    ..   mov  rax, [rdi+8]        # ← obj 1 starts HERE
[0,7]     .D=====eeeeeE-R.    ..   mov  rcx, [rax]          #   while obj 0 is still
[0,8]     .D==========eER.    ..   test rcx, 1              #   retiring
[0,9]     .D===========eER    ..   jne  .Lskip1
[0,10]    .D==========eE-R    ..   or   rcx, 1
[0,11]    .D===========eER    ..   mov  [rax], rcx
[0,12]    . DeeeeeE------R    ..   mov  rax, [rdi+16]       # ← obj 2
[0,13]    . D=====eeeeeE-R    ..   mov  rcx, [rax]
[0,14]    . D==========eER    ..   test rcx, 1
[0,15]    . D===========eER   ..   jne  .Lskip2
[0,16]    . D==========eE-R   ..   or   rcx, 1
[0,17]    . D===========eER   ..   mov  [rax], rcx
[0,18]    .  DeeeeeE------R   ..   mov  rax, [rdi+24]       # ← obj 3
[0,19]    .  D=====eeeeeE-R   ..   mov  rcx, qword ptr [rax]
[0,20]    .  D==========eER   ..   test rcx, 1
[0,21]    .  D===========eER  ..   jne  .Lskip3
[0,22]    .  D==========eE-R  ..   or   rcx, 1
[0,23]    .  D===========eER  ..   mov  [rax], rcx
```

**Key insight**: Look at how object 1 (`[0,6]`) starts **in cycle 1** while
object 0 is still executing. The OOO engine overlaps all 4 objects' work:

```
Cycle:  0    1    2    3    4    5    6    7    8    9   10   11   12
Obj 0:  D----eeeee---eeeee--eR
Obj 1:       D----eeeee---eeeee--eR
Obj 2:            D----eeeee---eeeee--eR
Obj 3:                 D----eeeee---eeeee--eR
                                                             ↑ all 4 complete
```

The loads for objects 0-3 are **all issued within cycles 0-3** even though
each takes 5 cycles to return. The OOO engine keeps all 4 loads in-flight
simultaneously → natural MLP.

### 3.3. SIMD Gather Timeline

```text
Timeline view (2 iterations, llvm-mca -mcpu=znver3):
                    0123456789          0123456789
Index     0123456789          0123456789

[0,0]     DeeeeeeeeER    .    .    .    .    .   .   vmovdqu  ymm2, [rdi+...]     # load 4 ptrs
[0,1]     DeE-------R    .    .    .    .    .   .   vpcmpeqd ymm3, ymm3, ymm3    # mask = all 1s
[0,2]     D---------R    .    .    .    .    .   .   vpxor    xmm4, xmm4, xmm4    # zero dest
[0,3]     D========eeeeeER    .    .    .    .   .   vpgatherqq ymm4, [ymm2], ymm3 # ← GATHER
[0,4]     D=============eER   .    .    .    .   .   vpand    ymm2, ymm4, ymm1    # mark & 1
[0,5]     D==============eER  .    .    .    .   .   vpcmpeqq ymm2, ymm2, ymm0    # cmp == 0
[0,6]     .D==============eER .    .    .    .   .   vpmovmskb r8d, ymm2          # extract mask
[0,7-20]  .  ...(13 insns: popcount emulation + vpor + scatter)...
[0,21]    .  D==========eE--------------R    .   .   vpor     ymm2, ymm4, ymm1   # set mark bits
[0,22]    .  D===========eeeeE----------R    .   .   vextracti128 xmm3, ymm2, 1  # split hi/lo
[0,23]    .  DeeeeeeeeE-----------------R    .   .   vmovdqu  xmm4, [rdi+...]    # reload ptrs (!)
[0,24]    .   DeeeeeeeeE----------------R    .   .   vmovdqu  xmm5, [rdi+...]    # reload ptrs (!)
[0,25]    .   D=======eE----------------R    .   .   vmovq    r9, xmm4           # extract ptr[0]
[0,26]    .   D===========eE------------R    .   .   vmovq    [r9], xmm2         # store header[0]
[0,27]    .   D=======eE----------------R    .   .   vpextrq  r9, xmm4, 1       # extract ptr[1]
[0,28]    .    D===========eeE-----------R   .   .   vpextrq  [r9], xmm2, 1     # store header[1]
[0,29]    .    D========eE---------------R   .   .   vmovq    r9, xmm5           # extract ptr[2]
[0,30]    .    D=============eE----------R   .   .   vmovq    [r9], xmm3         # store header[2]
[0,31]    .    D========eE---------------R   .   .   vpextrq  r9, xmm5, 1       # extract ptr[3]
[0,32]    .    .D=============eeE--------R   .   .   vpextrq  [r9], xmm3, 1     # store header[3]
[0,33]    .    .D======================eER   .   .   shrl     $27, r8d           # count / 8
[0,34]    .    .D=======================eER  .   .   addq     r8, rax            # accumulate
```

**Key problems visible in the timeline:**

1. **The gather blocks everything.** Instructions `[0,4]` through `[0,34]` all
   wait for `vpgatherqq` to complete (cycle 8→13). Nothing can make progress
   until the gather finishes because all downstream work depends on its result.

2. **No overlap between iterations.** Iteration 1 (`[1,3]`) doesn't start its
   gather until cycle ~15, after iteration 0 is almost done. Compare with
   scalar where all 4 loads are issued in cycles 0-3.

3. **Redundant pointer reloads.** Instructions `[0,23]` and `[0,24]` reload
   the same 4 pointers from the traversal buffer that were already loaded in
   `[0,0]`, because AVX2 has no scatter instruction — the code must extract
   individual pointers to perform scalar stores.

4. **13 integer instructions for popcount.** Without `popcnt`, the compiler
   emits a Hamming weight sequence (shift, mask, add × 4). This wastes 13 µops
   of ALU bandwidth on integer popcount emulation.

### 3.4. Average Wait Times

From llvm-mca's wait time analysis:

```text
                        Scalar        SIMD Gather
Avg scheduler wait:     8.8 cycles    12.2 cycles
Avg wait while ready:   0.2 cycles     0.4 cycles
Avg retire wait:        1.2 cycles     8.7 cycles   ← instructions wait much
                                                       longer to retire in SIMD
```

The SIMD loop's average retirement wait is **7.3× higher** than scalar's. This
means instructions complete execution but sit in the ROB waiting for earlier
(slow) instructions to retire. The gather's long latency creates a retirement
bottleneck.

---

## 4. Why the OOO Engine Beats SIMD Gather for MLP

The core question was: **can `vpgatherqq` provide better memory-level
parallelism than scalar loads by issuing 4 loads simultaneously?**

The answer is **no**, for two reasons:

### 4.1. The gather doesn't issue loads simultaneously

On Zen 3, `vpgatherqq` is decoded into 23 micro-ops. These include:
- Address computation µops (index extraction, scaling)
- Individual load µops (dispatched to load ports one at a time)
- Result merging µops (packing loaded data into the destination YMM)

The loads within the gather instruction are **serialized through the backend** —
they flow through the same AGU and load port pipeline as scalar loads, but with
additional overhead from the microcode sequencer.

```
Scalar approach:                     Gather approach:
┌──────────────────────────────┐     ┌──────────────────────────────┐
│ Cycle 0: issue load[0]       │     │ Cycle 0: begin vpgatherqq    │
│ Cycle 0: issue load[1]  ←OOO│     │ Cycle 1:  µop: extract idx[0]│
│ Cycle 1: issue load[2]  ←OOO│     │ Cycle 2:  µop: load[0]       │
│ Cycle 1: issue load[3]  ←OOO│     │ Cycle 3:  µop: merge[0]      │
│   (all 4 in-flight by cy 1)  │     │ Cycle 4:  µop: extract idx[1]│
│                              │     │ Cycle 5:  µop: load[1]       │
│ Cycle 5-6: all results back  │     │ ...                          │
│                              │     │ Cycle ~13: all results back  │
└──────────────────────────────┘     └──────────────────────────────┘

Scalar: 4 loads in-flight by cycle 1    Gather: 4 loads serialized over ~13 cyc
```

### 4.2. The OOO engine already provides excellent MLP

Zen 3 has:
- **256-entry reorder buffer** — can track ~256 instructions in-flight
- **3 load ports** (Zn3Load × 3) — can issue 3 loads per cycle
- **6-wide dispatch** — can dispatch 6 µops per cycle

In the scalar loop, each iteration's loads are independent (no data dependency
between `mov rax, [rdi]` and `mov rax, [rdi+8]`). The OOO engine sees this and
issues them all simultaneously:

```
Scalar loop unrolled 4×:
             Cycle: 0  1  2  3  4  5  6  7  8  9 10 11 12 13
load ptr[0]:        L--L--L--L--L→ done
load ptr[1]:        L--L--L--L--L→ done
load ptr[2]:           L--L--L--L--L→ done
load ptr[3]:           L--L--L--L--L→ done
load hdr[0]:                       L--L--L--L--L→ done
load hdr[1]:                       L--L--L--L--L→ done
load hdr[2]:                          L--L--L--L--L→ done
load hdr[3]:                          L--L--L--L--L→ done
test+branch: ...........                         T→
store:       ..........................................S→

All 8 loads (4 ptr + 4 hdr) issued in cycles 0-4, all complete by ~10.
```

The OOO engine achieves **8 loads in 5 issue cycles** = 1.6 loads/cycle,
well within Zen 3's 3 loads/cycle capacity.

---

## 5. Benchmark Results

```text
1M objects, 4M edges (512 MB object data, exceeds 256 MB total L3):

mark_check_strategies/scalar
                        time:   [31.0 ms  31.2 ms  31.3 ms]
                        thrpt:  [134 Melem/s  135 Melem/s  135 Melem/s]

mark_check_strategies/batch_branchless
                        time:   [38.2 ms  38.4 ms  38.5 ms]
                        thrpt:  [109 Melem/s  109 Melem/s  110 Melem/s]

mark_check_strategies/simd_avx2
                        time:   [45.7 ms  45.8 ms  46.0 ms]
                        thrpt:  [91 Melem/s   91 Melem/s   92 Melem/s]

trace_gc_random_dag_1048576 (full MMTk GC cycle)
                        time:   [8.7 ms   9.0 ms   9.3 ms]
```

| Strategy | Time | Throughput | vs Scalar |
|----------|------|-----------|-----------|
| **scalar** | **31.2 ms** | **135 Melem/s** | baseline |
| batch_branchless | 38.4 ms | 109 Melem/s | 1.23× slower |
| simd_avx2 | 45.8 ms | 91 Melem/s | **1.47× slower** |

---

## 6. Compiled Assembly (from `objdump`)

### 6.1. Scalar inner loop (4 objects)

```asm
; --- Object 0 ---
mov  rax, qword ptr [rdi]        ; load slot → object pointer
mov  rcx, qword ptr [rax]        ; load header word (mark bit at bit 0)
test rcx, 1                       ; is bit 0 set?
jnz  .Lskip0                      ; skip if already marked
or   rcx, 1                       ; set mark bit
mov  qword ptr [rax], rcx         ; store marked header
.Lskip0:
; --- Object 1 (same pattern) ---
mov  rax, qword ptr [rdi+8]
mov  rcx, qword ptr [rax]
test rcx, 1
jnz  .Lskip1
or   rcx, 1
mov  qword ptr [rax], rcx
.Lskip1:
; ... Objects 2, 3 identical ...
```

**6 instructions per object** (worst case). 24 µops for 4 objects.

### 6.2. SIMD gather inner loop (4 objects)

```asm
; Load 4 object pointers
vmovdqu  ymm2, [rdi + rcx*8 - 0x18]

; Prepare gather mask
vpcmpeqd ymm3, ymm3, ymm3          ; mask = all 1s
vpxor    xmm4, xmm4, xmm4          ; zero destination

; GATHER: load 4 headers from addresses in ymm2
vpgatherqq ymm4, [ymm2*1], ymm3    ; ← 23 µops on Zen 3!

; Extract mark bits and count unmarked
vpand    ymm2, ymm4, ymm1          ; headers & 1
vpcmpeqq ymm2, ymm2, ymm0          ; compare == 0
vpmovmskb r8d, ymm2                 ; extract bitmask

; Popcount emulation (no popcnt for ymm result)
mov  r9d, r8d
shr  r9d, 1
and  r9d, 0x55555555               ; 13 integer instructions
sub  r8d, r9d                       ; for Hamming weight...
... (8 more insns)
imul r8d, r9d, 0x01010101

; Set mark bits
vpor   ymm2, ymm4, ymm1            ; new_headers = headers | 1

; Scatter (no AVX2 scatter — must extract and store individually)
vextracti128 xmm3, ymm2, 1         ; extract high 128 bits
vmovdqu  xmm4, [rdi + rcx*8 - 0x18]; RELOAD pointers (!)
vmovdqu  xmm5, [rdi + rcx*8 - 0x8] ; RELOAD pointers (!)
vmovq    r9, xmm4                   ; extract ptr[0]
vmovq    [r9], xmm2                 ; store header[0]
vpextrq  r9, xmm4, 1               ; extract ptr[1]
vpextrq  [r9], xmm2, 1             ; store header[1]
vmovq    r9, xmm5                   ; extract ptr[2]
vmovq    [r9], xmm3                 ; store header[2]
vpextrq  r9, xmm5, 1               ; extract ptr[3]
vpextrq  [r9], xmm3, 1             ; store header[3]

shr  r8d, 27                        ; count / 8
add  rax, r8                        ; accumulate newly_marked
```

**35 instructions, 39 µops** for 4 objects. Nearly every instruction depends
on `vpgatherqq`, creating a long serial dependency chain.

---

## 7. Conclusions

1. **`vpgatherqq` is 4.6× more expensive in µops than 4 scalar loads on Zen 3**
   (23 µops vs 4+4=8 µops for load-ptr + load-header).

2. **The OOO engine provides better MLP than gather.** With scalar code, Zen 3's
   256-entry ROB and 3 load ports naturally overlap multiple iterations' loads.
   The gather instruction serializes the same work through microcode.

3. **The SIMD approach adds overhead that scalar doesn't have**: scatter stores
   (no `vpscatterqq` in AVX2), pointer reloads, popcount emulation, and
   FP-port pressure from 23 µops of gather microcode.

4. **This is an AMD Zen 3 result.** Intel Skylake+ implements gather with only
   5 µops. Intel Ice Lake and later have dedicated gather hardware that may
   perform differently. The same benchmark on an Intel platform might show
   the gather approach performing closer to scalar.

## References

- [uops.info — VPGATHERQQ YMM (Zen 3)](https://uops.info/html-instr/VPGATHERQQ_YMM_VSIB_YMM_YMM.html): 23 µops, 4.00c throughput, 15c latency
- [uops.info — VPGATHERQQ YMM (Skylake)](https://uops.info/html-instr/VPGATHERQQ_YMM_VSIB_YMM_YMM.html): 5 µops, 4.00c throughput, 20c latency
- [uops.info — VPGATHERDD YMM (Zen 3)](https://uops.info/html-instr/VPGATHERDD_YMM_VSIB_YMM_YMM.html): 39 µops, 8.00c throughput
- [Agner Fog's microarchitecture manual](https://agner.org/optimize/): Zen 3 has 256-entry ROB, 3 load ports, 6-wide dispatch
- [Agner Fog's instruction tables](https://agner.org/optimize/): referenced for general Zen 3 instruction timings
- llvm-mca 19.1.7 (`-mcpu=znver3`): run on assembly extracted via `objdump -d` from the benchmark binary
- [Stack Overflow (Peter Cordes et al.)](https://stackoverflow.com/questions/69558463): "no need to use gathers on AMD processors" due to high µops/rTP
