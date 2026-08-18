import clsx from 'clsx';
import type { ReactNode } from 'react';
import SectionLabel from './SectionLabel';

interface PanelSectionProps {
  children: ReactNode;
  className?: string;
  label?: string;
}

// A bordered, padded block stacked below a `PanelHeader` — the
// Transfer/Duplicates/Import blocks in SummaryPanel and the Folder template
// block in LeftPanel were all hand-rolling this same
// `border-t border-surface p-3` shell with slightly different gaps and
// border colors before this existed.
export default function PanelSection({ children, className, label }: PanelSectionProps) {
  return (
    <div className={clsx('p-3 border-t border-surface shrink-0 flex flex-col gap-2', className)}>
      {label && <SectionLabel>{label}</SectionLabel>}
      {children}
    </div>
  );
}
