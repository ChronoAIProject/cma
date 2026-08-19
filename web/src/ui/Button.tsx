import type { ButtonHTMLAttributes, ReactNode } from 'react';
import './Button.css';

/**
 * The one button — `docs/Web_Style.md` §7.
 *
 * A capsule, not a rectangle. The variants are fill families rather than
 * loudness levels: `primary` is the one action a surface exists for, `ghost`
 * is everything else and is the default, `outline` is the answer that is not
 * the primary one beside it, and `toolbar` is a control sitting on chrome
 * rather than on a page.
 *
 * Height lives on the control and every control is `box-sizing: border-box`,
 * because `outline` carries a 1px border and the others do not: under
 * content-box the two render 2px apart and sit a pixel out of line in the
 * same row. That is not a thing anyone catches in review, and it is exactly
 * what a shared component is for.
 */

/** The fill family, each backed by its own token set. */
export type ButtonVariant = 'primary' | 'ghost' | 'outline' | 'toolbar';

/** `md` is the 36px capsule; `sm` the 28px one for dense rows. */
export type ButtonSize = 'md' | 'sm';

interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'className'> {
  readonly children?: ReactNode;
  readonly variant?: ButtonVariant;
  readonly size?: ButtonSize;
  /** A leading 16px glyph. Sized by the button so callers cannot drift. */
  readonly icon?: ReactNode;
  /** Extra classes for placement only. Never for colour, depth or size. */
  readonly placement?: string;
}

export function Button({
  children,
  variant = 'ghost',
  size = 'md',
  icon,
  placement,
  type = 'button',
  ...rest
}: ButtonProps) {
  /*
   * No reason prop, deliberately. §7: why a control cannot be used is stated
   * once per row, beside the controls it explains, and never stacked into
   * each one — which made every disabled control taller than its neighbour.
   */
  const classes = ['button', `button--${variant}`, `button--${size}`, placement ?? '']
    .filter(Boolean)
    .join(' ');

  return (
    <button type={type} className={classes} {...rest}>
      {icon === undefined ? null : (
        <span className="button__icon" aria-hidden="true">
          {icon}
        </span>
      )}
      {children}
    </button>
  );
}
