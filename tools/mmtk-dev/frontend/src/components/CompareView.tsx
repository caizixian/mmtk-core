import React, { useState, useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';
import { fetchJSON, formatDiff, diffClass, statusBadge, type Baseline, type Run, type CompareResult, type MetricComparison, type ComparisonEntry } from '../api';
import { TH } from './ui';

export default function CompareView(): React.ReactElement {
    const [searchParams, setSearchParams] = useSearchParams();
    const [baselines, setBaselines] = useState<Baseline[]>([]);
    const [runs, setRuns] = useState<Run[]>([]);
    const [selectedBaseline, setSelectedBaseline] = useState(searchParams.get('baseline') || '');
    const [selectedRun, setSelectedRun] = useState(searchParams.get('run_id') || '');
    const [threshold, setThreshold] = useState(parseFloat(searchParams.get('threshold') || '0.02'));
    const [selectedMetric, setSelectedMetric] = useState(searchParams.get('metric') || '');
    const [availableMetrics, setAvailableMetrics] = useState<string[]>([]);
    const [result, setResult] = useState<CompareResult | null>(null);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => { loadOptions(); }, []);

    // Fetch available metrics when selected run changes
    useEffect(() => {
        if (selectedRun) {
            fetchJSON<string[]>(`/runs/${selectedRun}/metrics`)
                .then(setAvailableMetrics)
                .catch(() => setAvailableMetrics([]));
        }
    }, [selectedRun]);

    async function loadOptions(): Promise<void> {
        try {
            const [bl, r] = await Promise.all([
                fetchJSON<Baseline[]>('/baselines'),
                fetchJSON<Run[]>('/runs'),
            ]);
            setBaselines(bl);
            setRuns(r);
            // Use URL params if provided, otherwise defaults
            const urlBl = searchParams.get('baseline');
            const urlRun = searchParams.get('run_id');
            const urlMetric = searchParams.get('metric');
            if (urlBl) {
                setSelectedBaseline(urlBl);
            } else {
                const defaultBl = bl.find(b => b.is_default);
                if (defaultBl) setSelectedBaseline(defaultBl.id);
                else if (bl.length > 0) setSelectedBaseline(bl[0].id);
            }
            if (urlRun) {
                setSelectedRun(urlRun);
            } else if (r.length > 0) {
                setSelectedRun(r[0].id);
            }
            if (urlMetric) {
                setSelectedMetric(urlMetric);
            }
            // Auto-compare if URL params are set
            if (urlBl && urlRun) {
                const t = parseFloat(searchParams.get('threshold') || '0.02');
                let url = `/compare?baseline=${urlBl}&run_id=${urlRun}&threshold=${t}`;
                if (urlMetric) url += `&metric=${urlMetric}`;
                const data = await fetchJSON<CompareResult>(url);
                setResult(data);
            }
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        }
    }

    async function doCompare(): Promise<void> {
        if (!selectedBaseline || !selectedRun) return;
        setError(null);
        const params: Record<string, string> = {
            baseline: selectedBaseline,
            run_id: selectedRun,
            threshold: String(threshold),
        };
        if (selectedMetric) params.metric = selectedMetric;
        setSearchParams(params);
        try {
            let url = `/compare?baseline=${selectedBaseline}&run_id=${selectedRun}&threshold=${threshold}`;
            if (selectedMetric) url += `&metric=${selectedMetric}`;
            const data = await fetchJSON<CompareResult>(url);
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
                    <label className="text-[11px] font-semibold text-text-muted uppercase tracking-wider">Metric</label>
                    <select value={selectedMetric} onChange={e => setSelectedMetric(e.target.value)}
                        className="bg-surface-card border border-border rounded px-3 py-2 text-[13px] font-mono text-text-primary min-w-[140px] focus:outline-none focus:border-border-focus transition-colors">
                        <option value="">None</option>
                        <option value="all">All</option>
                        {availableMetrics.map(m => (
                            <option key={m} value={m}>{m}</option>
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
                <div className="space-y-6">
                    {/* Execution time comparison */}
                    <ComparisonTable
                        title="Execution Time"
                        unit="ms"
                        comparisons={result.comparisons}
                        geomeanDiff={result.geomean_diff}
                        geomeanChange={result.geomean_change}
                    />

                    {/* Metric comparisons (if requested) */}
                    {result.metric_comparisons && result.metric_comparisons.map(mc => (
                        <ComparisonTable
                            key={mc.metric}
                            title={`Metric: ${mc.metric}`}
                            unit={mc.metric.startsWith('time.') ? 'ms' : ''}
                            comparisons={mc.comparisons}
                            geomeanDiff={mc.geomean_diff}
                            geomeanChange={mc.geomean_change}
                        />
                    ))}
                </div>
            )}
        </div>
    );
}

/** Reusable comparison table component for both execution time and metrics. */
function ComparisonTable({
    title,
    unit,
    comparisons,
    geomeanDiff,
    geomeanChange,
}: {
    title: string;
    unit: string;
    comparisons: ComparisonEntry[];
    geomeanDiff: number;
    geomeanChange: string;
}): React.ReactElement {
    const pct = formatDiff(geomeanDiff);
    const cls = geomeanChange === 'faster'
        ? 'bg-emerald-400/10 text-emerald-400'
        : geomeanChange === 'slower'
        ? 'bg-red-400/10 text-red-400'
        : 'bg-indigo-500/10 text-text-secondary';
    const label = geomeanChange === 'faster' ? 'improvement'
        : geomeanChange === 'slower' ? 'regression' : 'neutral';
    const icon = geomeanChange === 'faster' ? '✅'
        : geomeanChange === 'slower' ? '❌' : '➡️';

    const unitLabel = unit ? ` (${unit})` : '';

    return (
        <div className="bg-surface-card border border-border rounded-lg overflow-hidden">
            {/* Geomean banner */}
            <div className={`px-5 py-4 text-[15px] font-semibold border-b border-border flex items-center gap-2 ${cls}`}>
                <span className="text-text-muted text-[12px] font-normal uppercase tracking-wider mr-2">{title}</span>
                {icon} Geometric mean: {pct} ({label})
            </div>
            <table className="w-full text-[13px]">
                <thead className="bg-surface">
                    <tr>
                        <TH>Benchmark</TH><TH>Baseline{unitLabel}</TH><TH>Target{unitLabel}</TH><TH>Diff</TH><TH>Status</TH>
                    </tr>
                </thead>
                <tbody>
                    {comparisons.map(c => {
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
    );
}
