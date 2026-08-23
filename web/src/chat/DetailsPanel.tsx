import type { ReactNode } from 'react';
import { IconCloseOutline16 } from '../ui/icons';
import './DetailsPanel.css';

/**
 * What a tool call actually did — `docs/Web_Style.md` §12.
 *
 * A tool call's arguments and its output used to open inside the row, in the
 * reading column. That is the wrong place for them: they are unbounded, they
 * are not prose, and they pushed the conversation they belong to off the
 * screen. Here they get a column of their own, with room to be read.
 *
 * **The panel is keyed by the call it is showing.** Its scroll position and
 * whatever it has open belong to that call, so moving to another one starts
 * fresh rather than inheriting a view of something else.
 *
 * A body that did not arrive draws nothing rather than an empty labelled
 * region — an absence is a fact, not a zero.
 */

export interface DetailsSection {
  readonly id: string;
  readonly label: string;
  readonly body: string;
  /** Whether the log stopped recording this body before it ended. */
  readonly truncated?: boolean;
  /** Whether it is the thing that went wrong. */
  readonly failed?: boolean;
}

interface DetailsPanelProps {
  /** What is being inspected. Absent when nothing is. */
  readonly title?: string;
  readonly sections: readonly DetailsSection[];
  readonly onClose: () => void;
  /** Drawn in place of the sections when there is nothing to show yet. */
  readonly note?: ReactNode;
}

export function DetailsPanel({ title, sections, onClose, note }: DetailsPanelProps) {
  return (
    <aside className="details" aria-label="Tool call details">
      <div className="details__head">
        <h2 className="details__title">{title ?? 'Details'}</h2>
        <button
          type="button"
          className="details__close"
          aria-label="Close details"
          onClick={onClose}
        >
          <IconCloseOutline16 size={14} />
        </button>
      </div>

      <div className="details__body">
        {sections.length === 0 ? (
          <p className="details__note">{note ?? 'Nothing to show for this call.'}</p>
        ) : (
          sections.map((section) => (
            <section className="details__section" key={section.id}>
              <p className="details__label">
                {section.label}
                {section.truncated === true ? ' (truncated)' : ''}
              </p>
              {/*
                Pre-wrapped rather than scrolled sideways: this column is as
                wide as somebody dragged it, and a horizontal scrollbar under
                every block is a second gesture for reading one thing.
              */}
              <pre className="details__block" data-error={section.failed === true ? '' : undefined}>
                {section.body}
              </pre>
            </section>
          ))
        )}
      </div>
    </aside>
  );
}
