import { useCallback, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';

const MIN_PANEL_WIDTH = 200;
const MAX_PANEL_WIDTH = 560;

/**
 * Same drag mechanics as RapidRAW's `createResizeHandler` in App.tsx
 * (pointer capture, live clamp during drag, cursor/user-select override for
 * the drag's duration) — adapted to a single panel with local state instead
 * of their global UI store, since we only ever resize two fixed panels.
 */
export function useResizablePanel(initialWidth: number, side: 'left' | 'right') {
  const [width, setWidth] = useState(initialWidth);
  const widthRef = useRef(width);
  widthRef.current = width;

  const onResizeStart = useCallback(
    (e: ReactPointerEvent<HTMLDivElement>) => {
      if (e.pointerType === 'mouse' && e.button !== 0) return;
      e.preventDefault();

      const pointerId = e.pointerId;
      const target = e.currentTarget;
      const startX = e.clientX;
      const startWidth = widthRef.current;

      const previousUserSelect = document.documentElement.style.userSelect;
      document.documentElement.style.userSelect = 'none';
      document.documentElement.style.cursor = 'col-resize';
      target.setPointerCapture?.(pointerId);

      const onMove = (moveEvent: PointerEvent) => {
        if (moveEvent.pointerId !== pointerId) return;
        moveEvent.preventDefault();
        const delta = moveEvent.clientX - startX;
        const raw = side === 'left' ? startWidth + delta : startWidth - delta;
        setWidth(Math.min(MAX_PANEL_WIDTH, Math.max(MIN_PANEL_WIDTH, raw)));
      };

      const onUp = (upEvent: PointerEvent) => {
        if (upEvent.pointerId !== pointerId) return;
        if (target.hasPointerCapture?.(pointerId)) target.releasePointerCapture(pointerId);
        document.documentElement.style.userSelect = previousUserSelect;
        document.documentElement.style.cursor = '';
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onUp);
        window.removeEventListener('pointercancel', onUp);
      };

      window.addEventListener('pointermove', onMove, { passive: false });
      window.addEventListener('pointerup', onUp);
      window.addEventListener('pointercancel', onUp);
    },
    [side],
  );

  return { width, onResizeStart };
}
