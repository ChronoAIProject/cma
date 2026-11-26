import type { ComponentPropsWithRef, ReactNode } from 'react';
import './Input.css';

/**
 * A single-line text field — the shared style contracts
 *
 * The border lives on the wrapper rather than the input, which is what lets a
 * leading glyph sit inside the field instead of beside it. The input itself
 * has no outline of its own: the focus ring is drawn on the wrapper, so a
 * field with an icon and one without focus identically.
 *
 * Not for the composer. A message box grows with what is typed into it and is
 * a different control with different rules; sharing this one would make every
 * search box growable.
 */

interface InputProps extends Omit<ComponentPropsWithRef<'input'>, 'className'> {
  /** A leading 16px glyph, drawn inside the field. */
  readonly icon?: ReactNode;
  /**
   * Extra class for placement only. Never for colour or height.
   * Sole exception: task-home landing controls (composer toolbar, send,
   * repository chip, templates, recovery actions) and repository dialog
   * controls, documented in Web_Style 2026-11-05.
   */
  readonly placement?: string;
}

export function Input({ icon, placement, ...rest }: InputProps) {
  return (
    <span className={['input', placement ?? ''].filter(Boolean).join(' ')}>
      {icon === undefined ? null : (
        <span className="input__icon" aria-hidden="true">
          {icon}
        </span>
      )}
      <input className="input__control" {...rest} />
    </span>
  );
}
