import React, { useState, useEffect } from 'react';
import RunsView from './components/RunsView';
import CompareView from './components/CompareView';
import TrendsView from './components/TrendsView';
import BaselinesView from './components/BaselinesView';
import { fetchJSON } from './api';

type ViewId = 'runs' | 'compare' | 'trends' | 'baselines';

interface NavItem {
    id: ViewId;
    label: string;
    icon: React.ReactNode;
}

const NAV_ITEMS: NavItem[] = [
    { id: 'runs', label: 'Runs', icon: (
        <svg className="w-[18px] h-[18px] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M9 5H7a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-2"/><rect x="9" y="3" width="6" height="4" rx="1"/><path d="M9 14l2 2 4-4"/></svg>
    )},
    { id: 'compare', label: 'Compare', icon: (
        <svg className="w-[18px] h-[18px] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M18 20V10"/><path d="M12 20V4"/><path d="M6 20v-6"/></svg>
    )},
    { id: 'trends', label: 'Trends', icon: (
        <svg className="w-[18px] h-[18px] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/></svg>
    )},
    { id: 'baselines', label: 'Baselines', icon: (
        <svg className="w-[18px] h-[18px] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/></svg>
    )},
];

export default function App(): React.ReactElement {
    const [view, setView] = useState<ViewId>('runs');
    const [healthy, setHealthy] = useState<boolean | null>(null);

    useEffect(() => {
        const check = async () => {
            try {
                const data = await fetchJSON<{ status: string }>('/health');
                setHealthy(data.status === 'ok');
            } catch {
                setHealthy(false);
            }
        };
        check();
        const interval = setInterval(check, 30000);
        return () => clearInterval(interval);
    }, []);

    return (
        <div className="flex h-full">
            {/* Sidebar */}
            <nav className="fixed inset-y-0 left-0 w-60 bg-surface border-r border-border flex flex-col z-10">
                <div className="px-5 pt-6 pb-4 flex items-baseline gap-2">
                    <h1 className="text-xl font-bold tracking-tight">
                        mmtk<span className="text-indigo-400">-dev</span>
                    </h1>
                    <span className="text-[11px] text-gray-500 font-mono">v0.1.0</span>
                </div>
                <ul className="flex-1 px-3 space-y-0.5">
                    {NAV_ITEMS.map(item => (
                        <li key={item.id}>
                            <button
                                onClick={() => setView(item.id)}
                                className={`w-full flex items-center gap-2.5 px-3 py-2.5 rounded-lg text-sm font-medium transition-colors ${
                                    view === item.id
                                        ? 'text-indigo-400 bg-indigo-500/10'
                                        : 'text-gray-400 hover:text-gray-200 hover:bg-surface-hover'
                                }`}
                            >
                                {item.icon}
                                {item.label}
                            </button>
                        </li>
                    ))}
                </ul>
                <div className="px-5 py-4 border-t border-border">
                    <div className="flex items-center gap-2 text-xs text-gray-500">
                        <span className={`w-2 h-2 rounded-full transition-colors ${
                            healthy === true ? 'bg-emerald-400 shadow-[0_0_6px] shadow-emerald-400'
                            : healthy === false ? 'bg-red-400'
                            : 'bg-yellow-400'
                        }`} />
                        <span>{healthy === true ? 'Connected' : healthy === false ? 'Disconnected' : 'Connecting...'}</span>
                    </div>
                </div>
            </nav>

            {/* Content */}
            <main className="ml-60 flex-1 p-8 overflow-y-auto">
                {view === 'runs' && <RunsView />}
                {view === 'compare' && <CompareView />}
                {view === 'trends' && <TrendsView />}
                {view === 'baselines' && <BaselinesView />}
            </main>
        </div>
    );
}
