import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Invokes } from '../utils/invokes';
import { Plan } from '../types/plan';

/**
 * Wraps the `scan_source` command. `scan()` itself still isn't
 * cancellable and returns the whole `Plan` in one shot (a known,
 * deliberately-deferred gap — see execution-plan.md), but it does now
 * emit `scan-progress` events with a running file count while it works,
 * same `listen`-before-`invoke` pattern as RapidRAW's `useThumbnails`.
 */
export interface ScannedFor {
  sourceRoot: string;
  destinationRoot: string;
  folderTemplate: string;
}

export function useScan() {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [scannedFor, setScannedFor] = useState<ScannedFor | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scannedCount, setScannedCount] = useState(0);

  const scan = useCallback(async (sourceRoot: string, destinationRoot: string, folderTemplate: string) => {
    setLoading(true);
    setError(null);
    setScannedCount(0);

    const unlisten = await listen<number>('scan-progress', (event) => {
      setScannedCount(event.payload);
    });

    try {
      const result = await invoke<Plan>(Invokes.ScanSource, {
        sourceRoot,
        destinationRoot,
        folderTemplate,
      });
      setPlan(result);
      setScannedFor({ sourceRoot, destinationRoot, folderTemplate });
      setScannedCount(result.items.length);
    } catch (err) {
      setError(String(err));
      setPlan(null);
      setScannedFor(null);
    } finally {
      setLoading(false);
      unlisten();
    }
  }, []);

  return { plan, scannedFor, loading, error, scannedCount, scan };
}
