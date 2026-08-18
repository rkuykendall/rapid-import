import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Invokes } from '../utils/invokes';
import { Plan, CommitSummary } from '../types/plan';

/**
 * Wraps the `commit_plan` command — the only place that actually
 * copies/moves files to disk. No progress events yet (unlike `useScan`'s
 * `scan-progress`): `commit.rs::commit_plan` has no progress callback of its
 * own, so this only ever reports "in flight" vs. "done."
 */
export function useCommit() {
  const [committing, setCommitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<CommitSummary | null>(null);

  const commit = useCallback(
    async (
      plan: Plan,
      destinationRoot: string,
      duplicatePolicy: string,
      transferMode: string,
      kind: 'import' | 'reorganize',
    ) => {
      setCommitting(true);
      setError(null);

      try {
        const result = await invoke<CommitSummary>(Invokes.CommitPlan, {
          plan,
          destinationRoot,
          duplicatePolicy,
          transferMode,
          kind,
        });
        setSummary(result);
        return result;
      } catch (err) {
        setError(String(err));
        return null;
      } finally {
        setCommitting(false);
      }
    },
    [],
  );

  const resetSummary = useCallback(() => setSummary(null), []);

  return { committing, error, summary, commit, resetSummary };
}
