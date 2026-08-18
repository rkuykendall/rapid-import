import type { ReactNode } from 'react';
import Text from './Text';
import { TextVariants } from '../../types/typography';

interface PanelHeaderProps {
  children: ReactNode;
  actions?: ReactNode;
}

// The title bar at the top of every `Panel` — bakes in `TextVariants.title`
// so every panel's heading renders at the same size by construction. Before
// this existed, each panel copy-pasted the wrapper and picked its own Text
// variant by hand, which is exactly how "Plan" ended up one size larger
// than "Setup"/"Summary" despite an otherwise identical header bar.
export default function PanelHeader({ children, actions }: PanelHeaderProps) {
  return (
    <div className="p-3 shrink-0 flex justify-between items-center border-b border-surface gap-4">
      <Text variant={TextVariants.title}>{children}</Text>
      {actions}
    </div>
  );
}
