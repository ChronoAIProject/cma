import type { ButtonHTMLAttributes, ReactNode } from 'react';
import './Pill.css';

/**
 * A small labelled chip — the shared style contracts
 *
 * A span until it is given something to do, and a button the moment it is.
 * That is not a convenience: a chip that reacts to a click while remaining a
 * span is unreachable by keyboard and unannounced to a screen reader, and a
 * chip that is always a button announces every static label as pressable.
 */

interface PillProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'className'> {
  readonly children?: ReactNode;
  /** Selected, for a pill that is one of a set. */
  readonly active?: boolean;
  /** Extra class for placement only. */
  readonly placement?: string;
}

export function Pill({ children, active = false, placement, onClick, ...rest }: PillProps) {
  const classes = ['pill', active ? 'pill--active' : '', placement ?? ''].filter(Boolean).join(' ');

  if (onClick === undefined) {
    return <span className={classes}>{children}</span>;
  }

  return (
    <button
      type="button"
      className={`${classes} pill--interactive`}
      onClick={onClick}
      aria-pressed={active}
      {...rest}
    >
      {children}
    </button>
  );
}
