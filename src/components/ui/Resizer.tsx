import clsx from 'clsx';
import type { PointerEventHandler } from 'react';

// RapidRAW defines this in a shared AppProperties.tsx alongside many
// editor-only enums we don't need — kept local here instead of pulling in
// that whole file for one value.
export enum Orientation {
  Horizontal = 'horizontal',
  Vertical = 'vertical',
}

interface ResizerProps {
  direction: Orientation;
  onMouseDown: PointerEventHandler<HTMLDivElement>;
}

const Resizer = ({ direction, onMouseDown }: ResizerProps) => (
  <div
    className={clsx('shrink-0 bg-transparent z-10 touch-none', {
      'w-2 cursor-col-resize': direction === Orientation.Vertical,
      'h-2 cursor-row-resize': direction === Orientation.Horizontal,
    })}
    role="separator"
    aria-orientation={direction === Orientation.Vertical ? 'vertical' : 'horizontal'}
    onPointerDown={onMouseDown}
    style={{ touchAction: 'none' }}
  />
);

export default Resizer;
