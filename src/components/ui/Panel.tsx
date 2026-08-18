import clsx from 'clsx';
import type { CSSProperties, ReactNode } from 'react';

interface PanelProps {
  children: ReactNode;
  className?: string;
  style?: CSSProperties;
}

// The card shell shared by all three top-level columns in App.tsx (left
// setup panel, center plan table, right summary panel) — sizing (fixed
// width vs. flex-1) stays the caller's concern via `className`/`style`.
export default function Panel({ children, className, style }: PanelProps) {
  return (
    <div className={clsx('bg-bg-secondary rounded-lg overflow-hidden', className)} style={style}>
      {children}
    </div>
  );
}
