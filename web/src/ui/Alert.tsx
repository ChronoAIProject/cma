import type { HTMLAttributes, ReactNode } from 'react';
import { IconCheckOutline16, IconQuestionOutline14, IconWarningOutline16 } from './icons';
import './Alert.css';

export type AlertTone = 'info' | 'success' | 'warning' | 'error';

/**
 * A message about the resource on screen — `docs/Web_Style.md` §12.1.
 *
 * Severity is drawn, not implied: the tone sets the glyph, edge and ground,
 * so a routine "Loading history" no longer shares the danger tint of a
 * failure. Information and success are polite status updates; a warning or
 * error interrupts. Actions sit at the end of the same row.
 */
export function Alert({
  tone,
  title,
  children,
  actions,
  ...rest
}: {
  readonly tone: AlertTone;
  readonly title?: ReactNode;
  readonly children?: ReactNode;
  readonly actions?: ReactNode;
} & Omit<HTMLAttributes<HTMLDivElement>, 'title' | 'role' | 'className' | 'children'>) {
  // Rest carries an id and the attributes a parent such as Presence adds.
  return (
    <div
      {...rest}
      className="alert"
      data-tone={tone}
      role={tone === 'warning' || tone === 'error' ? 'alert' : 'status'}
    >
      <span className="alert__icon" aria-hidden="true">
        {tone === 'success' ? (
          <IconCheckOutline16 />
        ) : tone === 'info' ? (
          <IconQuestionOutline14 size={16} />
        ) : (
          <IconWarningOutline16 />
        )}
      </span>
      <div className="alert__text">
        {title === undefined ? null : <p className="alert__title">{title}</p>}
        {children === undefined || children === null ? null : (
          <div className="alert__body">{children}</div>
        )}
      </div>
      {actions === undefined || actions === null ? null : (
        <div className="alert__actions">{actions}</div>
      )}
    </div>
  );
}
