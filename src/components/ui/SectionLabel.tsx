import clsx from 'clsx';
import type { ReactNode } from 'react';
import Text from './Text';
import { TextVariants } from '../../types/typography';

interface SectionLabelProps {
  children: ReactNode;
  className?: string;
}

// The small uppercase heading used above a group of controls (e.g.
// "Transfer", "Destinations") — previously re-typed as
// `variant={TextVariants.small} className="uppercase tracking-wide"` at
// every call site.
export default function SectionLabel({ children, className }: SectionLabelProps) {
  return (
    <Text variant={TextVariants.small} className={clsx('uppercase tracking-wide', className)}>
      {children}
    </Text>
  );
}
