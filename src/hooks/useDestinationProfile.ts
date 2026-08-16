import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Invokes } from '../utils/invokes';
import { Profile } from '../types/profile';

const LOAD_DEBOUNCE_MS = 400;

/**
 * Once a destination (library) is picked, recalls the source folder and
 * folder template last used *for that destination* — mirrors RapidRAW's
 * pick-a-folder-first flow, rather than one global "last used" state that
 * can't tell two different libraries apart. `save()` is only ever called
 * explicitly (on Scan), not on every keystroke.
 */
export function useDestinationProfile(destinationRoot: string, initialFolderTemplate: string) {
  const [sourceRoot, setSourceRoot] = useState('');
  const [folderTemplate, setFolderTemplate] = useState(initialFolderTemplate);

  useEffect(() => {
    if (destinationRoot.trim() === '') return;

    let cancelled = false;
    const timer = setTimeout(() => {
      invoke<Profile | null>(Invokes.LoadProfileForDestination, { destinationRoot })
        .then((profile) => {
          if (cancelled || !profile) return;
          setSourceRoot(profile.source_root ?? '');
          setFolderTemplate(profile.folder_template);
        })
        .catch(() => {
          // No history for this destination (or the lookup failed) — leave
          // whatever's currently in the fields alone.
        });
    }, LOAD_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [destinationRoot]);

  const save = useCallback(
    (sourceRootOverride?: string) => {
      if (destinationRoot.trim() === '') return Promise.resolve();
      return invoke(Invokes.SaveProfileForDestination, {
        sourceRoot: sourceRootOverride ?? sourceRoot,
        destinationRoot,
        folderTemplate,
      }).catch(() => {
        // Best-effort — losing a "remember this for next time" write isn't
        // worth surfacing as a user-facing error.
      });
    },
    [sourceRoot, destinationRoot, folderTemplate],
  );

  return { sourceRoot, setSourceRoot, folderTemplate, setFolderTemplate, save };
}
