import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Invokes } from '../utils/invokes';
import { IndexSummary } from '../types/index_library';

/**
 * Wraps the `refresh_library_index` command — builds/refreshes the SQLite
 * index from what's actually on disk under a destination (or a subfolder of
 * it), so `scan`/`already_imported` can recognize content that was never
 * imported *through* this app. Explicit and user-triggered only; a plain
 * scan never calls this itself. Mirrors `useScan`'s `listen`-before-`invoke`
 * progress pattern.
 */
export function useIndexLibrary() {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [indexedCount, setIndexedCount] = useState(0);
  const [summary, setSummary] = useState<IndexSummary | null>(null);

  const refreshIndex = useCallback(async (destinationRoot: string, scopeRoot?: string) => {
    setLoading(true);
    setError(null);
    setIndexedCount(0);
    setSummary(null);

    const unlisten = await listen<number>('index-progress', (event) => {
      setIndexedCount(event.payload);
    });

    try {
      const result = await invoke<IndexSummary>(Invokes.RefreshLibraryIndex, {
        destinationRoot,
        scopeRoot: scopeRoot || null,
      });
      setSummary(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
      unlisten();
    }
  }, []);

  return { loading, error, indexedCount, summary, refreshIndex };
}
