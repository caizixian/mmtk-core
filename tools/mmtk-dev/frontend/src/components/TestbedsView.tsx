import React, { useState, useEffect } from 'react';
import { fetchJSON, formatTime } from '../api';

interface Testbed {
    id: string;
    name: string;
    cpu_model: string;
    cpu_cores: number;
    memory_gb: number;
    created_at: string;
}

function InfoCard({ label, value, sub }: { label: string; value: string; sub?: string }): React.ReactElement {
    return (
        <div className="bg-surface-card border border-border rounded-lg p-4">
            <div className="text-[11px] font-semibold text-text-muted uppercase tracking-wider mb-1">{label}</div>
            <div className="text-lg font-bold text-text-primary font-mono">{value}</div>
            {sub && <div className="text-[12px] text-text-secondary mt-0.5">{sub}</div>}
        </div>
    );
}

export default function TestbedsView(): React.ReactElement {
    const [testbeds, setTestbeds] = useState<Testbed[]>([]);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => { loadTestbeds(); }, []);

    async function loadTestbeds(): Promise<void> {
        try {
            setLoading(true);
            const data = await fetchJSON<Testbed[]>('/testbeds');
            setTestbeds(data);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }

    if (loading) return <div className="text-text-muted italic py-12 text-center">Loading...</div>;
    if (error) return <div className="text-red-400 italic py-12 text-center">Error: {error}</div>;

    return (
        <div>
            <div className="mb-6">
                <h2 className="text-2xl font-bold tracking-tight">Testbeds</h2>
                <p className="text-sm text-text-secondary mt-1">Machines used for benchmark execution</p>
            </div>

            {testbeds.length === 0 && (
                <div className="bg-surface-card border border-border rounded-lg p-8 text-center text-text-muted italic">
                    No testbeds registered. Run a benchmark to auto-detect your machine.
                </div>
            )}

            {testbeds.map(tb => (
                <div key={tb.id} className="mb-6">
                    <div className="flex items-center gap-3 mb-4">
                        <svg className="w-5 h-5 text-indigo-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                            <rect x="2" y="3" width="20" height="14" rx="2"/>
                            <path d="M8 21h8"/>
                            <path d="M12 17v4"/>
                        </svg>
                        <h3 className="text-lg font-semibold">{tb.name || tb.id}</h3>
                        <span className="text-[12px] font-mono text-text-muted bg-surface border border-border px-2 py-0.5 rounded">{tb.id}</span>
                    </div>

                    <div className="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
                        <InfoCard label="CPU Model" value={tb.cpu_model || 'Unknown'} />
                        <InfoCard label="CPU Cores" value={tb.cpu_cores ? String(tb.cpu_cores) : 'N/A'} />
                        <InfoCard
                            label="Memory"
                            value={tb.memory_gb ? `${tb.memory_gb.toFixed(1)} GB` : 'N/A'}
                        />
                        <InfoCard
                            label="Registered"
                            value={formatTime(tb.created_at)}
                        />
                    </div>
                </div>
            ))}
        </div>
    );
}
