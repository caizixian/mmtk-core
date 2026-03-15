#!/usr/bin/env python3
"""Query and analyze collapsed-stack flamegraph data from async-profiler.

Usage:
    python3 flamegraph_query.py <command> [options] <collapsed_file>

Commands:
    summary         Show thread/category breakdown
    top             Show top functions by self (leaf) or inclusive time
    callers         Show callers of a function
    callees         Show callees of a function
    filter          Filter stacks matching a pattern and show top functions
    diff            Compare two collapsed files

Examples:
    # Overall summary
    python3 flamegraph_query.py summary profile.txt

    # Top 20 leaf functions in GC workers
    python3 flamegraph_query.py top --thread gc --mode self -n 20 profile.txt

    # Top inclusive functions across all threads
    python3 flamegraph_query.py top --mode inclusive -n 15 profile.txt

    # Who calls trace_object?
    python3 flamegraph_query.py callers trace_object profile.txt

    # What does trace_object call?
    python3 flamegraph_query.py callees trace_object profile.txt

    # Filter to only stacks containing "alloc" and show breakdown
    python3 flamegraph_query.py filter alloc profile.txt

    # Diff two profiles
    python3 flamegraph_query.py diff baseline.txt optimized.txt
"""

import sys
import argparse
import re
from collections import defaultdict
from pathlib import Path


def parse_collapsed(path):
    """Parse collapsed stack format: frame1;frame2;...;leafN count"""
    stacks = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            parts = line.rsplit(' ', 1)
            if len(parts) != 2:
                continue
            frames_str, count_str = parts
            try:
                count = int(count_str)
            except ValueError:
                continue
            frames = frames_str.split(';')
            stacks.append((frames, count))
    return stacks


def classify_thread(frames):
    """Classify a stack by thread type."""
    joined = ';'.join(frames[:10])
    if 'start_worker' in joined:
        return 'gc'
    elif 'MMTkMutatorContext::alloc' in ';'.join(frames):
        return 'alloc'
    else:
        return 'app'


def filter_stacks(stacks, thread=None, pattern=None):
    """Filter stacks by thread type and/or pattern."""
    result = []
    for frames, count in stacks:
        if thread:
            t = classify_thread(frames)
            if thread == 'gc' and t != 'gc':
                continue
            if thread == 'app' and t == 'gc':
                continue
            if thread == 'alloc' and 'alloc_slow_inline' not in ';'.join(frames):
                continue
        if pattern:
            if not any(pattern.lower() in f.lower() for f in frames):
                continue
        result.append((frames, count))
    return result


def cmd_summary(stacks, args):
    """Show overall breakdown."""
    total = sum(c for _, c in stacks)
    gc = sum(c for f, c in stacks if classify_thread(f) == 'gc')
    alloc_slow = sum(c for f, c in stacks if 'alloc_slow_inline' in ';'.join(f))
    block_gc = sum(c for f, c in stacks if 'block_for_gc' in ';'.join(f) or 'mmtk_block_for_gc' in ';'.join(f))

    print(f"Total samples: {total}")
    print()
    print("=== THREAD BREAKDOWN ===")
    print(f"  GC workers:           {gc:6d} ({100*gc/total:5.1f}%)")
    print(f"  Application threads:  {total-gc:6d} ({100*(total-gc)/total:5.1f}%)")
    print()
    print("=== APPLICATION THREAD BREAKDOWN ===")
    print(f"  Alloc slow path:      {alloc_slow:6d} ({100*alloc_slow/total:5.1f}%)")
    print(f"    Blocked on GC:      {block_gc:6d} ({100*block_gc/total:5.1f}%)")
    print(f"  Non-alloc app work:   {total-gc-alloc_slow:6d} ({100*(total-gc-alloc_slow)/total:5.1f}%)")
    print()

    # Top mmtk functions
    print("=== TOP MMTk FUNCTIONS (self/leaf, all threads) ===")
    leaf = defaultdict(int)
    for frames, count in stacks:
        f = frames[-1]
        if 'mmtk' in f.lower() or 'mmtk_openjdk' in f.lower():
            leaf[f] += count
    for func, cnt in sorted(leaf.items(), key=lambda x: -x[1])[:15]:
        print(f"  {100*cnt/total:5.2f}%  {cnt:5d}  {func}")


def cmd_top(stacks, args):
    """Show top functions by self or inclusive time."""
    filtered = filter_stacks(stacks, thread=args.thread, pattern=args.pattern)
    total_filtered = sum(c for _, c in filtered)

    if total_filtered == 0:
        print("No matching stacks found.")
        return

    counts = defaultdict(int)
    if args.mode == 'self':
        for frames, count in filtered:
            counts[frames[-1]] += count
    else:  # inclusive
        for frames, count in filtered:
            seen = set()
            for f in frames:
                if f not in seen:
                    counts[f] += count
                    seen.add(f)

    n = args.n or 20
    print(f"Top {n} functions ({args.mode}), {total_filtered} samples"
          + (f", thread={args.thread}" if args.thread else "")
          + (f", pattern={args.pattern}" if args.pattern else ""))
    print()
    for func, cnt in sorted(counts.items(), key=lambda x: -x[1])[:n]:
        print(f"  {100*cnt/total_filtered:5.1f}%  {cnt:5d}  {func}")


def cmd_callers(stacks, args):
    """Show immediate callers of a function."""
    pattern = args.function.lower()
    callers = defaultdict(int)
    total_calls = 0

    for frames, count in stacks:
        for i, f in enumerate(frames):
            if pattern in f.lower() and i > 0:
                callers[frames[i-1]] += count
                total_calls += count

    if total_calls == 0:
        print(f"No stacks contain '{args.function}'")
        return

    n = args.n or 15
    print(f"Callers of '{args.function}' ({total_calls} samples)")
    print()
    for func, cnt in sorted(callers.items(), key=lambda x: -x[1])[:n]:
        print(f"  {100*cnt/total_calls:5.1f}%  {cnt:5d}  {func}")


def cmd_callees(stacks, args):
    """Show immediate callees of a function."""
    pattern = args.function.lower()
    callees = defaultdict(int)
    total_calls = 0

    for frames, count in stacks:
        for i, f in enumerate(frames):
            if pattern in f.lower() and i < len(frames) - 1:
                callees[frames[i+1]] += count
                total_calls += count

    if total_calls == 0:
        print(f"No stacks contain '{args.function}'")
        return

    n = args.n or 15
    print(f"Callees of '{args.function}' ({total_calls} samples)")
    print()
    for func, cnt in sorted(callees.items(), key=lambda x: -x[1])[:n]:
        print(f"  {100*cnt/total_calls:5.1f}%  {cnt:5d}  {func}")


def cmd_filter(stacks, args):
    """Filter stacks by pattern and show top functions."""
    filtered = filter_stacks(stacks, thread=args.thread, pattern=args.function)
    total = sum(c for _, c in stacks)
    total_filtered = sum(c for _, c in filtered)

    if total_filtered == 0:
        print(f"No stacks match '{args.function}'")
        return

    print(f"Stacks matching '{args.function}': {total_filtered} samples ({100*total_filtered/total:.1f}% of total)")
    print()

    # Self
    leaf = defaultdict(int)
    for frames, count in filtered:
        leaf[frames[-1]] += count

    n = args.n or 15
    print(f"Top {n} leaf functions:")
    for func, cnt in sorted(leaf.items(), key=lambda x: -x[1])[:n]:
        print(f"  {100*cnt/total_filtered:5.1f}%  {cnt:5d}  {func}")

    print()

    # Inclusive
    incl = defaultdict(int)
    for frames, count in filtered:
        seen = set()
        for f in frames:
            if f not in seen:
                incl[f] += count
                seen.add(f)

    print(f"Top {n} inclusive functions:")
    for func, cnt in sorted(incl.items(), key=lambda x: -x[1])[:n]:
        print(f"  {100*cnt/total_filtered:5.1f}%  {cnt:5d}  {func}")


def cmd_diff(stacks_a, stacks_b, args):
    """Compare two profiles."""
    def leaf_counts(stacks):
        c = defaultdict(int)
        for frames, count in stacks:
            c[frames[-1]] += count
        return c

    a = leaf_counts(stacks_a)
    b = leaf_counts(stacks_b)
    total_a = sum(a.values())
    total_b = sum(b.values())

    all_funcs = set(a.keys()) | set(b.keys())
    diffs = []
    for f in all_funcs:
        pct_a = 100 * a.get(f, 0) / total_a if total_a else 0
        pct_b = 100 * b.get(f, 0) / total_b if total_b else 0
        diffs.append((f, pct_a, pct_b, pct_b - pct_a))

    n = args.n or 20
    print(f"Diff: {total_a} samples (A) vs {total_b} samples (B)")
    print()
    print(f"Top {n} by absolute change:")
    print(f"  {'Δ%':>6s}  {'A%':>5s}  {'B%':>5s}  Function")
    for func, pa, pb, d in sorted(diffs, key=lambda x: -abs(x[3]))[:n]:
        sign = '+' if d > 0 else ''
        print(f"  {sign}{d:.2f}%  {pa:.2f}%  {pb:.2f}%  {func}")


def main():
    parser = argparse.ArgumentParser(description='Query collapsed-stack flamegraph data')
    sub = parser.add_subparsers(dest='command', help='Command')

    p_summary = sub.add_parser('summary', help='Overall breakdown')
    p_summary.add_argument('file', help='Collapsed stack file')

    p_top = sub.add_parser('top', help='Top functions')
    p_top.add_argument('file', help='Collapsed stack file')
    p_top.add_argument('--mode', choices=['self', 'inclusive'], default='self')
    p_top.add_argument('--thread', choices=['gc', 'app', 'alloc'], default=None)
    p_top.add_argument('--pattern', '-p', default=None, help='Filter to stacks containing pattern')
    p_top.add_argument('-n', type=int, default=20, help='Number of results')

    p_callers = sub.add_parser('callers', help='Show callers of a function')
    p_callers.add_argument('function', help='Function name (substring match)')
    p_callers.add_argument('file', help='Collapsed stack file')
    p_callers.add_argument('-n', type=int, default=15)

    p_callees = sub.add_parser('callees', help='Show callees of a function')
    p_callees.add_argument('function', help='Function name (substring match)')
    p_callees.add_argument('file', help='Collapsed stack file')
    p_callees.add_argument('-n', type=int, default=15)

    p_filter = sub.add_parser('filter', help='Filter stacks by pattern')
    p_filter.add_argument('function', help='Pattern to match (substring)')
    p_filter.add_argument('file', help='Collapsed stack file')
    p_filter.add_argument('--thread', choices=['gc', 'app', 'alloc'], default=None)
    p_filter.add_argument('-n', type=int, default=15)

    p_diff = sub.add_parser('diff', help='Compare two profiles')
    p_diff.add_argument('file_a', help='Baseline collapsed stack file')
    p_diff.add_argument('file_b', help='Comparison collapsed stack file')
    p_diff.add_argument('-n', type=int, default=20)

    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        sys.exit(1)

    if args.command == 'diff':
        stacks_a = parse_collapsed(args.file_a)
        stacks_b = parse_collapsed(args.file_b)
        cmd_diff(stacks_a, stacks_b, args)
    else:
        stacks = parse_collapsed(args.file)
        if args.command == 'summary':
            cmd_summary(stacks, args)
        elif args.command == 'top':
            cmd_top(stacks, args)
        elif args.command == 'callers':
            cmd_callers(stacks, args)
        elif args.command == 'callees':
            cmd_callees(stacks, args)
        elif args.command == 'filter':
            cmd_filter(stacks, args)


if __name__ == '__main__':
    main()
