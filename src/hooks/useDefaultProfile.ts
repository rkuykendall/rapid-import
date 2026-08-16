import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Invokes } from '../utils/invokes';
import { Profile } from '../types/profile';

const SAVE_DEBOUNCE_MS = 500;

/**
 * Loads the single auto-persisted "default" profile on mount and
 * debounce-saves source/destination/template back to it on every change —
 * this is what makes the folder pickers survive an app restart. Backed by
 * the same `profiles` table a future named-profile-switcher would use.
 */
export function useDefaultProfile(initialFolderTemplate: string) {
  const [sourceRoot, setSourceRoot] = useState('');
  const [destinationRoot, setDestinationRoot] = useState('');
  const [folderTemplate, setFolderTemplate] = useState(initialFolderTemplate);
  const loaded = useRef(false);

  useEffect(() => {
    invoke<Profile | null>(Invokes.LoadDefaultProfile)
      .then((profile) => {
        if (profile) {
          setSourceRoot(profile.source_root ?? '');
          setDestinationRoot(profile.destination_root ?? '');
          setFolderTemplate(profile.folder_template);
        }
      })
      .finally(() => {
        loaded.current = true;
      });
  }, []);

  useEffect(() => {
    // Skip the save that would otherwise fire from the initial (empty)
    // render, before the load above has had a chance to populate state —
    // that would immediately overwrite a previously-saved profile.
    if (!loaded.current) return;

    const timer = setTimeout(() => {
      invoke(Invokes.SaveDefaultProfile, { sourceRoot, destinationRoot, folderTemplate }).catch(() => {
        // Best-effort — losing a "remember this for next time" write isn't
        // worth surfacing as a user-facing error.
      });
    }, SAVE_DEBOUNCE_MS);

    return () => clearTimeout(timer);
  }, [sourceRoot, destinationRoot, folderTemplate]);

  return { sourceRoot, setSourceRoot, destinationRoot, setDestinationRoot, folderTemplate, setFolderTemplate };
}
