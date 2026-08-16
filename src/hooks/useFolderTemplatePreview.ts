import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Invokes } from '../utils/invokes';

const DEBOUNCE_MS = 200;

/**
 * Renders `template` against "now" via the same `render_template` the
 * scan engine actually uses, debounced — this is the format guide: rather
 * than us validating/documenting chrono's strftime syntax ourselves, the
 * user just sees what their template produces in real time.
 */
export function useFolderTemplatePreview(template: string): string {
  const [preview, setPreview] = useState('');

  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(() => {
      invoke<string>(Invokes.PreviewFolderTemplate, { folderTemplate: template })
        .then((result) => {
          if (!cancelled) setPreview(result);
        })
        .catch(() => {
          if (!cancelled) setPreview('');
        });
    }, DEBOUNCE_MS);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [template]);

  return preview;
}
