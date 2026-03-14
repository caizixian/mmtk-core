import React from 'react';
import { Link } from 'react-router-dom';
import { formatTime, type Baseline } from '../api';
import { useFetch } from '../hooks';
import { TH, TD } from './ui';

export default function BaselinesView(): React.ReactElement {
    const { data: baselines, loading, error } = useFetch<Baseline[]>('/baselines');

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
                            <TH>Name</TH><TH>Run ID</TH><TH>Default</TH><TH>Description</TH><TH>Created</TH>
                        </tr>
                    </thead>
                    <tbody>
                        {loading && (
                            <tr><td colSpan={5} className="px-4 py-12 text-center text-text-muted italic">Loading...</td></tr>
                        )}
                        {error && (
                            <tr><td colSpan={5} className="px-4 py-12 text-center text-red-400 italic">Error: {error}</td></tr>
                        )}
                        {!loading && !error && baselines?.length === 0 && (
                            <tr><td colSpan={5} className="px-4 py-12 text-center text-text-muted italic">
                                No baselines set. Use <span className="font-mono text-indigo-400">mmtk-dev set-baseline</span> to create one.
                            </td></tr>
                        )}
                        {baselines?.map(bl => (
                            <tr key={bl.id} className="hover:bg-surface-hover transition-colors">
                                <TD className="font-semibold">{bl.id}</TD>
                                <TD className="text-text-secondary"><Link to={`/runs/${bl.run_id}`} className="text-indigo-400 hover:text-indigo-300 transition-colors">{bl.run_id}</Link></TD>
                                <TD>
                                    {bl.is_default ? (
                                        <span className="inline-flex items-center px-2.5 py-0.5 rounded-full text-[11px] font-semibold bg-indigo-500/10 text-indigo-400">★ default</span>
                                    ) : null}
                                </TD>
                                <TD className="text-text-secondary">{bl.description || '-'}</TD>
                                <TD className="text-text-secondary">{formatTime(bl.created_at)}</TD>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </div>
        </div>
    );
}
