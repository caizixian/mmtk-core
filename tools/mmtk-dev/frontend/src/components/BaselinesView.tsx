import React, { useState, useEffect } from 'react';
import { fetchJSON, formatTime, type Baseline } from '../api';

export default function BaselinesView(): React.ReactElement {
    const [baselines, setBaselines] = useState<Baseline[]>([]);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => { loadBaselines(); }, []);

    async function loadBaselines(): Promise<void> {
        try {
            setLoading(true);
            const data = await fetchJSON<Baseline[]>('/baselines');
            setBaselines(data);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }

    return (
        <div>
            <div className="mb-6">
                <h2 className="text-2xl font-bold tracking-tight">Baselines</h2>
                <p className="text-sm text-text-secondary mt-1">Named baselines for performance comparisons</p>
            </div>

            <div className="bg-surface-card border border-border rounded-lg overflow-hidden">
                <table className="w-full text-[13px]">
                    <thead className="bg-surface">
                        <tr>
                            <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">Name</th>
                            <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">Run ID</th>
                            <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">Default</th>
                            <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">Description</th>
                            <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">Created</th>
                        </tr>
                    </thead>
                    <tbody>
                        {loading && (
                            <tr><td colSpan={5} className="px-4 py-12 text-center text-text-muted italic">Loading...</td></tr>
                        )}
                        {error && (
                            <tr><td colSpan={5} className="px-4 py-12 text-center text-red-400 italic">Error: {error}</td></tr>
                        )}
                        {!loading && !error && baselines.length === 0 && (
                            <tr><td colSpan={5} className="px-4 py-12 text-center text-text-muted italic">
                                No baselines set. Use <span className="font-mono text-indigo-400">mmtk-dev set-baseline</span> to create one.
                            </td></tr>
                        )}
                        {baselines.map(bl => (
                            <tr key={bl.id} className="hover:bg-surface-hover transition-colors">
                                <td className="px-4 py-2.5 border-b border-border font-mono text-[12.5px] font-semibold">{bl.id}</td>
                                <td className="px-4 py-2.5 border-b border-border font-mono text-[12.5px] text-text-secondary">{bl.run_id}</td>
                                <td className="px-4 py-2.5 border-b border-border">
                                    {bl.is_default ? (
                                        <span className="inline-flex items-center px-2.5 py-0.5 rounded-full text-[11px] font-semibold bg-indigo-500/10 text-indigo-400">★ default</span>
                                    ) : null}
                                </td>
                                <td className="px-4 py-2.5 border-b border-border text-text-secondary text-[12.5px]">{bl.description || '-'}</td>
                                <td className="px-4 py-2.5 border-b border-border font-mono text-[12.5px] text-text-secondary">{formatTime(bl.created_at)}</td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </div>
        </div>
    );
}
