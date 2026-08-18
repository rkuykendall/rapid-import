import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Invokes } from '../utils/invokes';
import { Plan, DuplicateGroup } from '../types/plan';

// Mirrors `ScanResult` in src-tauri/src/main.rs — `scan_source`'s actual
// response shape (the plan plus every exact/near-duplicate pairing
// `dedup::find_duplicates` found), bundled into one round-trip.
interface ScanResult {
  plan: Plan;
  duplicates: DuplicateGroup[];
}

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
  const [duplicates, setDuplicates] = useState<DuplicateGroup[]>([]);
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
      const result = await invoke<ScanResult>(Invokes.ScanSource, {
        sourceRoot,
        destinationRoot,
        folderTemplate,
      });
      setPlan(result.plan);
      setDuplicates(result.duplicates);
      setScannedFor({ sourceRoot, destinationRoot, folderTemplate });
      setScannedCount(result.plan.items.length);
    } catch (err) {
      setError(String(err));
      setPlan(null);
      setDuplicates([]);
      setScannedFor(null);
    } finally {
      setLoading(false);
      unlisten();
    }
  }, []);

  // Client-side only — `excluded` just governs what `commit_plan` skips at
  // commit time (see `commit.rs`'s `commit_item`, checked ahead of any
  // conflict handling), so there's nothing to recompute or re-invoke here.
  const toggleExcluded = useCallback((sourcePath: string) => {
    setPlan((current) => {
      if (!current) return current;
      return {
        items: current.items.map((item) =>
          item.source_path === sourcePath ? { ...item, excluded: !item.excluded } : item,
        ),
      };
    });
  }, []);

  return { plan, duplicates, scannedFor, loading, error, scannedCount, scan, toggleExcluded };
}
