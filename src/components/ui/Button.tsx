import clsx from 'clsx';
import type { ButtonHTMLAttributes, ReactNode } from 'react';

interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children' | 'size'> {
  children: ReactNode;
  size?: string;
  variant?: string;
}

const Button = ({
  children,
  onClick,
  disabled,
  className = '',
  size: _size,
  variant: _variant,
  ...props
}: ButtonProps) => {
  const baseClasses = `
    flex items-center justify-center gap-2
    font-semibold py-2 px-4 rounded-md
    text-md
    disabled:opacity-50 disabled:cursor-not-allowed
  `;

  const hasSurfaceBg = className.includes('bg-surface');

  // `text-button-text` is tuned to contrast with `bg-accent` (e.g. black
  // text on the Dark theme's white accent) — on `bg-surface` it's the wrong
  // color entirely (black-on-dark-surface, effectively invisible), so the
  // surface variant needs the same text color the rest of the UI uses on
  // surface backgrounds instead.
  const combinedClasses = clsx(
    baseClasses,
    {
      'bg-accent text-button-text': !hasSurfaceBg,
      'bg-surface text-text-primary': hasSurfaceBg,
    },
    className,
  );

  return (
    <button onClick={onClick} disabled={disabled} className={combinedClasses} {...props}>
      {children}
    </button>
  );
};

export default Button;
