import React, { useState, useEffect } from 'react';
import { fetchJSON, formatTime, type Build, type Run, type BenchmarkResult, type Stats } from '../api';

function TH({ children }: { children: React.ReactNode }): React.ReactElement {
    return <th className="px-4 py-3 text-left text-[11px] font-semibold text-gray-400 uppercase tracking-wider border-b border-border">{children}</th>;
}

function TD({ children, className = '' }: { children: React.ReactNode; className?: string }): React.ReactElement {
    return <td className={`px-4 py-2.5 border-b border-border font-mono text-[12.5px] ${className}`}>{children}</td>;
}

export default function RunsView(): React.ReactElement {
    const [runs, setRuns] = useState<Run[]>([]);
    const [builds, setBuilds] = useState<Record<string, Build>>({});
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [detailRun, setDetailRun] = useState<string | null>(null);
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
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }

    async function showDetail(runId: string): Promise<void> {
        setDetailRun(runId);
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
                <p className="text-sm text-gray-400 mt-1">Recent benchmark runs with per-benchmark statistics</p>
            </div>

            <div className="bg-surface-card border border-border rounded-lg overflow-hidden">
                <table className="w-full text-[13px]">
                    <thead className="bg-surface">
                        <tr>
                            <TH>Run ID</TH><TH>Build</TH><TH>Plan</TH>
                            <TH>Invocations</TH><TH>Status</TH><TH>Started</TH><TH>Actions</TH>
                        </tr>
                    </thead>
                    <tbody>
                        {loading && (
                            <tr><td colSpan={7} className="px-4 py-12 text-center text-gray-500 italic">Loading...</td></tr>
                        )}
                        {error && (
                            <tr><td colSpan={7} className="px-4 py-12 text-center text-red-400 italic">Error: {error}</td></tr>
                        )}
                        {!loading && !error && runs.length === 0 && (
                            <tr><td colSpan={7} className="px-4 py-12 text-center text-gray-500 italic">
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
                                    <TD>
                                        <span className={`inline-flex items-center px-2.5 py-0.5 rounded-full text-[11px] font-semibold ${statusCls}`}>
                                            {run.status}
                                        </span>
                                    </TD>
                                    <TD className="text-gray-400">{formatTime(run.started_at)}</TD>
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
                        <button onClick={() => { setDetailRun(null); setDetailData(null); }}
                            className="text-gray-400 hover:text-gray-200 px-2 py-1 rounded hover:bg-surface-hover transition-colors">✕</button>
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
                                        <TD className="text-gray-400">{s.ci?.toFixed(1) ?? '-'}</TD>
                                        <TD>{s.median?.toFixed(1) ?? '-'}</TD>
                                        <TD className="text-gray-400">{s.stdev?.toFixed(2) ?? '-'}</TD>
                                        <TD>{s.n ?? 0}</TD>
                                    </tr>
                                );
                            })}
                        </tbody>
                    </table>
                </div>
            )}
        </div>
    );
}
