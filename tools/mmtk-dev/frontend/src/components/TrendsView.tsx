import React, { useState, useEffect, useRef, useCallback } from 'react';
import { fetchJSON, type TrendPoint } from '../api';

export default function TrendsView(): React.ReactElement {
    const [benchmarks, setBenchmarks] = useState<string[]>([]);
    const [selected, setSelected] = useState('');
    const [trendsData, setTrendsData] = useState<Record<string, TrendPoint[]>>({});
    const canvasRef = useRef<HTMLCanvasElement>(null);

    useEffect(() => { loadData(); }, []);

    async function loadData(): Promise<void> {
        try {
            const data = await fetchJSON<Record<string, TrendPoint[]>>('/trends');
            setBenchmarks(Object.keys(data).sort());
            setTrendsData(data);
        } catch { /* silently fail */ }
    }

    const drawChart = useCallback((bm: string) => {
        const canvas = canvasRef.current;
        if (!canvas || !trendsData[bm]) return;

        const ctx = canvas.getContext('2d');
        if (!ctx) return;

        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        canvas.width = rect.width * dpr;
        canvas.height = 350 * dpr;
        ctx.scale(dpr, dpr);

        const w = rect.width;
        const h = 350;
        const pad = { top: 40, right: 30, bottom: 50, left: 70 };
        const plotW = w - pad.left - pad.right;
        const plotH = h - pad.top - pad.bottom;

        ctx.clearRect(0, 0, w, h);

        // Read colors from CSS variables for theme-awareness
        const style = getComputedStyle(document.documentElement);
        const INDIGO = '#818cf8';
        const GRID = style.getPropertyValue('--chart-grid').trim() || '#2a2d42';
        const TEXT = style.getPropertyValue('--chart-text').trim() || '#8b8fa8';

        const data = [...trendsData[bm]].reverse().filter(p => p.mean != null);
        if (data.length === 0) return;

        const means = data.map(d => d.mean!);
        const cis = data.map(d => d.ci || 0);
        const minY = Math.min(...means.map((m, i) => m - cis[i])) * 0.95;
        const maxY = Math.max(...means.map((m, i) => m + cis[i])) * 1.05;

        const xScale = (i: number) => pad.left + (i / Math.max(data.length - 1, 1)) * plotW;
        const yScale = (v: number) => pad.top + plotH - ((v - minY) / (maxY - minY)) * plotH;

        // Grid
        ctx.strokeStyle = GRID;
        ctx.lineWidth = 0.5;
        for (let i = 0; i <= 5; i++) {
            const y = pad.top + (i / 5) * plotH;
            ctx.beginPath();
            ctx.moveTo(pad.left, y);
            ctx.lineTo(w - pad.right, y);
            ctx.stroke();
            const val = maxY - (i / 5) * (maxY - minY);
            ctx.fillStyle = TEXT;
            ctx.font = '11px "JetBrains Mono"';
            ctx.textAlign = 'right';
            ctx.fillText(val.toFixed(0), pad.left - 10, y + 4);
        }

        // CI band
        ctx.fillStyle = 'rgba(129, 140, 248, 0.1)';
        ctx.beginPath();
        data.forEach((d, i) => {
            const x = xScale(i), y = yScale(d.mean! + (d.ci || 0));
            i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
        });
        for (let i = data.length - 1; i >= 0; i--) {
            ctx.lineTo(xScale(i), yScale(data[i].mean! - (data[i].ci || 0)));
        }
        ctx.closePath();
        ctx.fill();

        // Line
        ctx.strokeStyle = INDIGO;
        ctx.lineWidth = 2;
        ctx.lineJoin = 'round';
        ctx.beginPath();
        data.forEach((d, i) => {
            const x = xScale(i), y = yScale(d.mean!);
            i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
        });
        ctx.stroke();

        // Points
        const BG = style.getPropertyValue('--chart-bg').trim() || '#1c1f2e';
        data.forEach((d, i) => {
            const x = xScale(i), y = yScale(d.mean!);
            ctx.fillStyle = BG;
            ctx.beginPath();
            ctx.arc(x, y, 4, 0, Math.PI * 2);
            ctx.fill();
            ctx.strokeStyle = INDIGO;
            ctx.lineWidth = 2;
            ctx.stroke();
        });

        // X labels
        ctx.fillStyle = TEXT;
        ctx.font = '10px "JetBrains Mono"';
        ctx.textAlign = 'center';
        const step = Math.max(1, Math.floor(data.length / 10));
        data.forEach((d, i) => {
            if (i % step !== 0 && i !== data.length - 1) return;
            ctx.save();
            ctx.translate(xScale(i), h - pad.bottom + 16);
            ctx.rotate(-Math.PI / 6);
            ctx.fillText(d.run_id.slice(0, 8), 0, 0);
            ctx.restore();
        });

        // Title
        ctx.fillStyle = INDIGO;
        ctx.font = 'bold 14px Inter';
        ctx.textAlign = 'left';
        ctx.fillText(`${bm} — Execution Time (ms)`, pad.left, pad.top - 16);
    }, [trendsData]);

    useEffect(() => {
        if (selected) drawChart(selected);
    }, [selected, drawChart]);

    return (
        <div>
            <div className="mb-6">
                <h2 className="text-2xl font-bold tracking-tight">Performance Trends</h2>
                <p className="text-sm text-text-secondary mt-1">Execution time trends across recent runs</p>
            </div>

            <div className="flex items-end gap-4 mb-6">
                <div className="flex flex-col gap-1.5">
                    <label className="text-[11px] font-semibold text-text-muted uppercase tracking-wider">Benchmark</label>
                    <select value={selected} onChange={e => setSelected(e.target.value)}
                        className="bg-surface-card border border-border rounded px-3 py-2 text-[13px] font-mono text-text-primary min-w-[180px] focus:outline-none focus:border-border-focus transition-colors">
                        <option value="">Select a benchmark...</option>
                        {benchmarks.map(bm => <option key={bm} value={bm}>{bm}</option>)}
                    </select>
                </div>
            </div>

            <div className="bg-surface-card border border-border rounded-lg p-5 relative">
                <canvas ref={canvasRef} className="w-full" height={350} />
                {!selected && (
                    <div className="absolute inset-0 flex items-center justify-center text-text-muted italic">
                        Select a benchmark to view its performance trend
                    </div>
                )}
            </div>
        </div>
    );
}
