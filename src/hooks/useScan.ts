import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Invokes } from '../utils/invokes';
import { Plan } from '../types/plan';

/**
 * Wraps the `scan_source` command — a single synchronous invoke for now.
 * `scan()` in the core crate isn't progressive/cancellable yet (a known,
 * deliberately-deferred gap — see execution-plan.md), so there's no
 * `scan-progress` event to listen for the way `useThumbnails` does for
 * thumbnails; this hook just tracks loading/error state around one call.
 */
export function useScan() {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scan = useCallback(async (sourceRoot: string, destinationRoot: string, folderTemplate: string) => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<Plan>(Invokes.ScanSource, {
        sourceRoot,
        destinationRoot,
        folderTemplate,
      });
      setPlan(result);
    } catch (err) {
      setError(String(err));
      setPlan(null);
    } finally {
      setLoading(false);
    }
  }, []);

  return { plan, loading, error, scan };
}
