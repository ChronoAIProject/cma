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
 * Heights come from §7's control scale: `sm` and the default for rows and
 * bands, `block` for a panel's one full-width primary action, and `lg` for
 * the landing's opening action, which takes the `--control-h-xl` step.
 */

/** What the button is for, which decides how loudly it is drawn. */
export type ButtonTone = 'quiet' | 'accent' | 'danger';

/** Which of §7's heights the control takes. */
export type ButtonSize = 'default' | 'sm' | 'block' | 'lg';

interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'className'> {
  readonly children: ReactNode;
  readonly tone?: ButtonTone;
  readonly size?: ButtonSize;
  /** Extra classes for placement only. Never for colour, depth or size. */
  readonly placement?: string;
}

const TONE: Record<ButtonTone, string> = {
  // A quiet control is the default. §7 spends the one primary button a
  // surface is allowed on the action that surface exists for, so it is never
  // the fallback.
  quiet: '',
  accent: 'button--primary',
  danger: 'button--danger',
};

const SIZE: Record<ButtonSize, string> = {
  default: '',
  sm: 'button--sm',
  block: 'button--block',
  lg: 'button--xl',
};

export function Button({
  children,
  tone = 'quiet',
  size = 'default',
  placement,
  type = 'button',
  disabled = false,
  ...rest
}: ButtonProps) {
  /*
   * No reason prop, deliberately. §7: why a control cannot be used is stated
   * once per row, beside the controls it explains, as a sibling
   * `.control-reason` drawn by the row — never stacked into each key, which
   * made every disabled control taller than its enabled neighbour.
   */
  const classes = ['button', TONE[tone], SIZE[size], placement ?? ''].filter(Boolean).join(' ');

  return (
    <button type={type} className={classes} disabled={disabled} {...rest}>
      {children}
    </button>
  );
}
