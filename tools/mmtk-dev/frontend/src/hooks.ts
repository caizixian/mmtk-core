import { useState, useEffect, useCallback } from 'react';
import { fetchJSON } from './api';

interface FetchState<T> {
    data: T | null;
    loading: boolean;
    error: string | null;
    reload: () => void;
}

/**
 * Hook for fetching JSON data from the API.
 * Handles loading/error states and provides a reload function.
 */
export function useFetch<T>(path: string): FetchState<T> {
    const [data, setData] = useState<T | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);

    const load = useCallback(async () => {
        try {
            setLoading(true);
            setError(null);
            const result = await fetchJSON<T>(path);
            setData(result);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }, [path]);

    useEffect(() => { load(); }, [load]);

    return { data, loading, error, reload: load };
}
