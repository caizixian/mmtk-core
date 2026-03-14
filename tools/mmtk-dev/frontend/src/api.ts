const API_BASE = '/api';

export interface Stats {
    n: number;
    mean: number | null;
    median: number | null;
    stdev: number | null;
    ci: number | null;
    min: number | null;
    max: number | null;
}

export interface Build {
    id: string;
    core_repo: string;
    core_commit: string;
    binding_repo: string;
    binding_commit: string;
    gc_plan: string;
    build_profile: string;
    core_branch?: string;
    created_at?: string;
}

export interface Run {
    id: string;
    build_id: string;
    testbed_id: string;
    invocations: number;
    heap_multiplier: number | null;
    status: string;
    started_at?: string;
    finished_at?: string;
}

export interface Baseline {
    id: string;
    run_id: string;
    is_default: number;
    description?: string;
    created_at?: string;
}

export interface ComparisonEntry {
    benchmark: string;
    baseline: Stats;
    target: Stats;
    diff: number | null;
    change: string;
}

export interface CompareResult {
    baseline_name: string;
    baseline_run_id: string;
    target_run_id: string;
    comparisons: ComparisonEntry[];
    geomean_diff: number;
    geomean_change: string;
}

export interface BenchmarkResult {
    stats: Stats;
    results: Record<string, unknown>[];
}

export interface TrendPoint {
    run_id: string;
    mean: number | null;
    ci: number | null;
    date: string | null;
}

export async function fetchJSON<T>(path: string): Promise<T> {
    const resp = await fetch(`${API_BASE}${path}`);
    if (!resp.ok) throw new Error(`API error: ${resp.status}`);
    return resp.json();
}

export function formatTime(iso: string | null | undefined): string {
    if (!iso) return '-';
    return new Date(iso).toLocaleString();
}

export function formatDiff(diff: number | null): string {
    if (diff == null) return '-';
    const pct = (diff * 100).toFixed(2);
    return `${diff > 0 ? '+' : ''}${pct}%`;
}

export function diffClass(change: string): string {
    if (change === 'faster') return 'text-emerald-400';
    if (change === 'slower') return 'text-red-400';
    return 'text-text-muted';
}

export function statusBadge(change: string): { text: string; cls: string } {
    if (change === 'faster') return { text: '✅ faster', cls: 'bg-emerald-400/10 text-emerald-400' };
    if (change === 'slower') return { text: '❌ slower', cls: 'bg-red-400/10 text-red-400' };
    if (change === 'no_data') return { text: '⚠ no data', cls: 'bg-yellow-400/10 text-yellow-400' };
    return { text: '➡️ neutral', cls: 'bg-indigo-500/10 text-text-secondary' };
}
