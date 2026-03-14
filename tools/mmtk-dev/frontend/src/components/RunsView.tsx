import React, { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { fetchJSON, formatTime, type Build, type Run, type BenchmarkResult, type Stats } from '../api';

function TH({ children }: { children: React.ReactNode }): React.ReactElement {
    return <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">{children}</th>;
}

function TD({ children, className = '' }: { children: React.ReactNode; className?: string }): React.ReactElement {
    return <td className={`px-4 py-2.5 border-b border-border font-mono text-[12.5px] ${className}`}>{children}</td>;
}

interface MetricRow {
    name: string;
    values: number[];
}

function MetricsTable({ results }: { results: Record<string, unknown>[] }): React.ReactElement {
    // Collect metrics across invocations
    const metricsMap: Record<string, number[]> = {};
    for (const r of results) {
        const metrics = r.metrics as Record<string, number>[] | undefined;
        if (metrics) {
            for (const m of metrics) {
                const name = m.name as unknown as string;
                const val = m.value as unknown as number;
                if (!metricsMap[name]) metricsMap[name] = [];
                metricsMap[name].push(val);
            }
        }
    }

    // Also look for top-level metric fields
    for (const r of results) {
        for (const [key, val] of Object.entries(r)) {
            if (key.startsWith('metric_') && typeof val === 'number') {
                const name = key.replace('metric_', '');
                if (!metricsMap[name]) metricsMap[name] = [];
                metricsMap[name].push(val);
            }
        }
    }

    const rows: MetricRow[] = Object.entries(metricsMap)
        .map(([name, values]) => ({ name, values }))
        .sort((a, b) => a.name.localeCompare(b.name));

    if (rows.length === 0) return <div className="px-5 py-4 text-text-muted italic text-sm">No MMTk metrics captured for this benchmark.</div>;

    return (
        <div className="mt-3 border-t border-border pt-3">
            <div className="px-5 mb-2 text-[11px] font-semibold text-text-muted uppercase tracking-wider">MMTk Statistics</div>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-2 px-5 pb-3">
                {rows.map(m => {
                    const avg = m.values.reduce((a, b) => a + b, 0) / m.values.length;
                    const displayVal = avg === Infinity || avg === -Infinity
                        ? '-'
                        : avg % 1 === 0
                        ? avg.toFixed(0)
                        : avg.toFixed(2);
                    return (
                        <div key={m.name} className="bg-surface border border-border rounded px-3 py-2">
                            <div className="text-[10px] text-text-muted font-mono truncate" title={m.name}>{m.name}</div>
                            <div className="text-sm font-semibold font-mono text-text-primary">{displayVal}</div>
                            {m.values.length > 1 && (
                                <div className="text-[10px] text-text-muted">({m.values.length} inv)</div>
                            )}
                        </div>
                    );
                })}
            </div>
        </div>
    );
}

export default function RunsView({ initialDetailRunId }: { initialDetailRunId?: string }): React.ReactElement {
    const navigate = useNavigate();
    const [runs, setRuns] = useState<Run[]>([]);
    const [builds, setBuilds] = useState<Record<string, Build>>({});
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [detailRun, setDetailRun] = useState<string | null>(initialDetailRunId ?? null);
    const [detailData, setDetailData] = useState<Record<string, BenchmarkResult> | null>(null);

    useEffect(() => { loadData(); }, []);

    async function loadData(): Promise<void> {
        try {
            setLoading(true);
            const [runsData, buildsData] = await Promise.all([
                fetchJSON<Run[]>('/runs'),
                fetchJSON<Build[]>('/builds'),
            ]);
            const buildsMap: Record<string, Build> = {};
            buildsData.forEach(b => { buildsMap[b.id] = b; });
            setRuns(runsData);
            setBuilds(buildsMap);
            // Auto-load detail if initialDetailRunId is set
            if (initialDetailRunId) {
                showDetail(initialDetailRunId);
            }
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }

    async function showDetail(runId: string, updateUrl = true): Promise<void> {
        setDetailRun(runId);
        if (updateUrl) navigate(`/runs/${runId}`);
        try {
            const data = await fetchJSON<Record<string, BenchmarkResult>>(`/runs/${runId}/results`);
            setDetailData(data);
        } catch (err) {
            setDetailData(null);
            setError(err instanceof Error ? err.message : 'Unknown error');
        }
    }

    return (
        <div>
            <div className="mb-6">
                <h2 className="text-2xl font-bold tracking-tight">Benchmark Runs</h2>
                <p className="text-sm text-text-secondary mt-1">Recent benchmark runs with per-benchmark statistics</p>
            </div>

            <div className="bg-surface-card border border-border rounded-lg overflow-hidden">
                <table className="w-full text-[13px]">
                    <thead className="bg-surface">
                        <tr>
                            <TH>Run ID</TH><TH>Build</TH><TH>Plan</TH>
                            <TH>Invocations</TH><TH>Heap</TH><TH>Status</TH><TH>Started</TH><TH>Actions</TH>
                        </tr>
                    </thead>
                    <tbody>
                        {loading && (
                            <tr><td colSpan={8} className="px-4 py-12 text-center text-text-muted italic">Loading...</td></tr>
                        )}
                        {error && (
                            <tr><td colSpan={8} className="px-4 py-12 text-center text-red-400 italic">Error: {error}</td></tr>
                        )}
                        {!loading && !error && runs.length === 0 && (
                            <tr><td colSpan={8} className="px-4 py-12 text-center text-text-muted italic">
                                No runs yet. Use <span className="font-mono text-indigo-400">mmtk-dev run</span> to create one.
                            </td></tr>
                        )}
                        {runs.map(run => {
                            const build = builds[run.build_id];
                            const commit = build?.core_commit?.slice(0, 8) || '?';
                            const plan = build?.gc_plan || '?';
                            const statusCls = run.status === 'completed'
                                ? 'bg-emerald-400/10 text-emerald-400'
                                : run.status === 'failed'
                                ? 'bg-red-400/10 text-red-400'
                                : 'bg-yellow-400/10 text-yellow-400';
                            return (
                                <tr key={run.id} className="hover:bg-surface-hover transition-colors">
                                    <TD>{run.id}</TD>
                                    <TD>{commit}</TD>
                                    <TD>{plan}</TD>
                                    <TD>{run.invocations || '-'}</TD>
                                    <TD className="text-text-secondary">{run.heap_multiplier ? `${run.heap_multiplier}x` : '-'}</TD>
                                    <TD>
                                        <span className={`inline-flex items-center px-2.5 py-0.5 rounded-full text-[11px] font-semibold ${statusCls}`}>
                                            {run.status}
                                        </span>
                                    </TD>
                                    <TD className="text-text-secondary">{formatTime(run.started_at)}</TD>
                                    <TD>
                                        <button onClick={() => showDetail(run.id)}
                                            className="text-indigo-400 hover:text-indigo-300 text-xs font-semibold transition-colors">
                                            Details
                                        </button>
                                    </TD>
                                </tr>
                            );
                        })}
                    </tbody>
                </table>
            </div>

            {detailRun && detailData && (
                <div className="bg-surface-card border border-border rounded-lg overflow-hidden mt-6">
                    <div className="flex justify-between items-center px-5 py-4 border-b border-border">
                        <h3 className="text-[15px] font-semibold">Run {detailRun}</h3>
                        <button onClick={() => { setDetailRun(null); setDetailData(null); navigate('/runs'); }}
                            className="text-text-muted hover:text-text-primary px-2 py-1 rounded hover:bg-surface-hover transition-colors">✕</button>
                    </div>
                    <table className="w-full text-[13px]">
                        <thead className="bg-surface">
                            <tr>
                                <TH>Benchmark</TH><TH>Mean (ms)</TH><TH>±CI</TH>
                                <TH>Median (ms)</TH><TH>Std Dev</TH><TH>n</TH>
                            </tr>
                        </thead>
                        <tbody>
                            {Object.keys(detailData).sort().map(bm => {
                                const s: Stats = detailData[bm].stats;
                                return (
                                    <tr key={bm} className="hover:bg-surface-hover transition-colors">
                                        <TD className="font-semibold">{bm}</TD>
                                        <TD>{s.mean?.toFixed(1) ?? '-'}</TD>
                                        <TD className="text-text-secondary">{s.ci?.toFixed(1) ?? '-'}</TD>
                                        <TD>{s.median?.toFixed(1) ?? '-'}</TD>
                                        <TD className="text-text-secondary">{s.stdev?.toFixed(2) ?? '-'}</TD>
                                        <TD>{s.n ?? 0}</TD>
                                    </tr>
                                );
                            })}
                        </tbody>
                    </table>
                    {/* MMTk Metrics per benchmark */}
                    {Object.keys(detailData).sort().map(bm => (
                        <MetricsTable key={bm} results={detailData[bm].results} />
                    ))}
                </div>
            )}
        </div>
    );
}
