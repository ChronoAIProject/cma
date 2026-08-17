import type { ButtonHTMLAttributes, ReactNode } from 'react';

/**
 * The one button — docs/Web_Style.md §7.
 *
 * One implementation rather than a class each surface hand-rolls. Height is
 * declared on the control and every control is `box-sizing: border-box`,
 * because a quiet button carries a 1px border and an accent button does not:
 * under content-box the two render 2px apart and sit a pixel out of line in
 * the same row. That is not a thing anyone notices in review, and it is
 * exactly the thing a shared component prevents.
 *
 * Two sizes and no others. `block` is a panel's one full-width primary action
 * and `lg` the landing's opening actions, both of which §7 names separately
 * from the two button heights.
 */

/** What the button is for, which decides how loudly it is drawn. */
export type ButtonTone = 'quiet' | 'accent' | 'danger';

/** Which of §7's heights the control takes. */
export type ButtonSize = 'default' | 'sm' | 'block' | 'lg';

interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'className'> {
  readonly children: ReactNode;
  readonly tone?: ButtonTone;
  readonly size?: ButtonSize;
  /**
   * Why this control cannot be used, stated as part of the button.
   *
   * §7: controls are disabled, never removed, and every disabled control says
   * why. A tooltip is unreachable by touch and invisible to anyone scanning,
   * so the reason is drawn — which is what turns the key into a stacked one,
   * holding a 52px minimum so a column of them stays even when a reason wraps.
   *
   * Given on an enabled control it describes what pressing it will do.
   */
  readonly reason?: string;
  /** Extra classes for placement only. Never for colour, depth or size. */
  readonly placement?: string;
}

const TONE: Record<ButtonTone, string> = {
  // A quiet control is the default. §7 spends the one accent button a view is
  // allowed on the action the view exists for, so it is never the fallback.
  quiet: 'press--quiet',
  accent: 'button--accent press--accent',
  danger: 'button--danger press--quiet',
};

const SIZE: Record<ButtonSize, string> = {
  default: '',
  sm: 'button--sm',
  block: 'button--block',
  lg: 'button--lg',
};

export function Button({
  children,
  tone = 'quiet',
  size = 'default',
  reason,
  placement,
  type = 'button',
  disabled = false,
  ...rest
}: ButtonProps) {
  const classes = [
    'button',
    TONE[tone],
    SIZE[size],
    reason === undefined ? '' : 'button--stacked',
    placement ?? '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <button type={type} className={classes} disabled={disabled} {...rest}>
      <span>{children}</span>
      {reason === undefined ? null : <span className="button__reason">{reason}</span>}
    </button>
  );
}
