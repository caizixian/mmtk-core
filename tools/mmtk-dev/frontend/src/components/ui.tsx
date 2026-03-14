import React from 'react';

export function TH({ children }: { children: React.ReactNode }): React.ReactElement {
    return <th className="px-4 py-3 text-left text-[11px] font-semibold text-text-muted uppercase tracking-wider border-b border-border">{children}</th>;
}

export function TD({ children, className = '' }: { children: React.ReactNode; className?: string }): React.ReactElement {
    return <td className={`px-4 py-2.5 border-b border-border font-mono text-[12.5px] ${className}`}>{children}</td>;
}
