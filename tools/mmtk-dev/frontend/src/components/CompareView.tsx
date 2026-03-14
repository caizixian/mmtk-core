import React, { useState, useEffect } from 'react';
import { fetchJSON, formatDiff, diffClass, statusBadge, type Baseline, type Run, type CompareResult } from '../api';

function TH({ children }: { children: React.ReactNode }): React.ReactElement {
    return <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">{children}</th>;
}

export default function CompareView(): React.ReactElement {
    const [baselines, setBaselines] = useState<Baseline[]>([]);
    const [runs, setRuns] = useState<Run[]>([]);
    const [selectedBaseline, setSelectedBaseline] = useState('');
    const [selectedRun, setSelectedRun] = useState('');
    const [threshold, setThreshold] = useState(0.02);
    const [result, setResult] = useState<CompareResult | null>(null);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => { loadOptions(); }, []);

    async function loadOptions(): Promise<void> {
        try {
            const [bl, r] = await Promise.all([
                fetchJSON<Baseline[]>('/baselines'),
                fetchJSON<Run[]>('/runs'),
            ]);
            setBaselines(bl);
            setRuns(r);
            const defaultBl = bl.find(b => b.is_default);
            if (defaultBl) setSelectedBaseline(defaultBl.id);
            else if (bl.length > 0) setSelectedBaseline(bl[0].id);
            if (r.length > 0) setSelectedRun(r[0].id);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        }
    }

    async function doCompare(): Promise<void> {
        if (!selectedBaseline || !selectedRun) return;
        setError(null);
        try {
            const data = await fetchJSON<CompareResult>(`/compare?baseline=${selectedBaseline}&run_id=${selectedRun}&threshold=${threshold}`);
            setResult(data);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        }
    }

    return (
        <div>
            <div className="mb-6">
                <h2 className="text-2xl font-bold tracking-tight">Compare</h2>
                <p className="text-sm text-text-secondary mt-1">Compare a run against a baseline</p>
            </div>

            <div className="flex items-end gap-4 mb-6 flex-wrap">
                <div className="flex flex-col gap-1.5">
                    <label className="text-[11px] font-semibold text-text-muted uppercase tracking-wider">Baseline</label>
                    <select value={selectedBaseline} onChange={e => setSelectedBaseline(e.target.value)}
                        className="bg-surface-card border border-border rounded px-3 py-2 text-[13px] font-mono text-text-primary min-w-[180px] focus:outline-none focus:border-border-focus transition-colors">
                        {baselines.length === 0 && <option value="">No baselines</option>}
                        {baselines.map(bl => (
                            <option key={bl.id} value={bl.id}>{bl.id}{bl.is_default ? ' ★' : ''}</option>
                        ))}
                    </select>
                </div>
                <div className="flex flex-col gap-1.5">
                    <label className="text-[11px] font-semibold text-text-muted uppercase tracking-wider">Target Run</label>
                    <select value={selectedRun} onChange={e => setSelectedRun(e.target.value)}
                        className="bg-surface-card border border-border rounded px-3 py-2 text-[13px] font-mono text-text-primary min-w-[180px] focus:outline-none focus:border-border-focus transition-colors">
                        {runs.length === 0 && <option value="">No runs</option>}
                        {runs.map(r => (
                            <option key={r.id} value={r.id}>{r.id}</option>
                        ))}
                    </select>
                </div>
                <div className="flex flex-col gap-1.5">
                    <label className="text-[11px] font-semibold text-text-muted uppercase tracking-wider">Threshold</label>
                    <input type="number" value={threshold} onChange={e => setThreshold(parseFloat(e.target.value) || 0.02)}
                        className="bg-surface-card border border-border rounded px-3 py-2 text-[13px] font-mono text-text-primary w-20 focus:outline-none focus:border-border-focus transition-colors"
                        step="0.01" min="0" max="1" />
                </div>
                <button onClick={doCompare}
                    className="bg-indigo-500 hover:bg-indigo-400 text-white px-4 py-2 rounded text-[13px] font-semibold transition-colors hover:shadow-lg hover:shadow-indigo-500/30">
                    Compare
                </button>
            </div>

            {error && <div className="bg-red-400/10 text-red-400 px-5 py-4 rounded-lg mb-6 text-sm">{error}</div>}

            {result && (
                <div className="bg-surface-card border border-border rounded-lg overflow-hidden">
                    {/* Geomean banner */}
                    {(() => {
                        const pct = formatDiff(result.geomean_diff);
                        const cls = result.geomean_change === 'faster'
                            ? 'bg-emerald-400/10 text-emerald-400'
                            : result.geomean_change === 'slower'
                            ? 'bg-red-400/10 text-red-400'
                            : 'bg-indigo-500/10 text-text-secondary';
                        const label = result.geomean_change === 'faster' ? 'improvement'
                            : result.geomean_change === 'slower' ? 'regression' : 'neutral';
                        const icon = result.geomean_change === 'faster' ? '✅'
                            : result.geomean_change === 'slower' ? '❌' : '➡️';
                        return (
                            <div className={`px-5 py-4 text-[15px] font-semibold border-b border-border flex items-center gap-2 ${cls}`}>
                                {icon} Geometric mean: {pct} ({label})
                            </div>
                        );
                    })()}
                    <table className="w-full text-[13px]">
                        <thead className="bg-surface">
                            <tr>
                                <TH>Benchmark</TH><TH>Baseline (ms)</TH><TH>Target (ms)</TH><TH>Diff</TH><TH>Status</TH>
                            </tr>
                        </thead>
                        <tbody>
                            {result.comparisons.map(c => {
                                const blStr = c.baseline.mean != null ? `${c.baseline.mean.toFixed(1)} ±${(c.baseline.ci || 0).toFixed(1)}` : '-';
                                const tgtStr = c.target.mean != null ? `${c.target.mean.toFixed(1)} ±${(c.target.ci || 0).toFixed(1)}` : '-';
                                const badge = statusBadge(c.change);
                                return (
                                    <tr key={c.benchmark} className="hover:bg-surface-hover transition-colors">
                                        <td className="px-4 py-2.5 border-b border-border font-mono text-[12.5px] font-semibold">{c.benchmark}</td>
                                        <td className="px-4 py-2.5 border-b border-border font-mono text-[12.5px]">{blStr}</td>
                                        <td className="px-4 py-2.5 border-b border-border font-mono text-[12.5px]">{tgtStr}</td>
                                        <td className={`px-4 py-2.5 border-b border-border font-mono text-[12.5px] ${diffClass(c.change)}`}>{formatDiff(c.diff)}</td>
                                        <td className="px-4 py-2.5 border-b border-border">
                                            <span className={`inline-flex items-center px-2.5 py-0.5 rounded-full text-[11px] font-semibold ${badge.cls}`}>{badge.text}</span>
                                        </td>
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
