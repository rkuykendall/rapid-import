import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Invokes } from '../utils/invokes';
import { Profile } from '../types/profile';

/**
 * Backs the Sources sidebar — every destination ever used. `refresh()` is
 * called explicitly after a scan (that's when `save_profile_for_destination`
 * actually writes), rather than polling.
 */
export function useProfiles() {
  const [profiles, setProfiles] = useState<Profile[]>([]);

  const refresh = useCallback(() => {
    invoke<Profile[]>(Invokes.ListProfiles)
      .then(setProfiles)
      .catch(() => {
        // Nothing saved yet, or the lookup failed — an empty sidebar is a
        // reasonable fallback either way.
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { profiles, refresh };
}
