import type { ReactNode } from 'react';
import type { PersonLabel } from './directory';

/**
 * The record list from docs/Web_Style.md §15, and the way a person is drawn
 * in one.
 *
 * Shared by all four views rather than reimplemented per view, because §15
 * states the row shape once — leading timestamp, primary label, secondary
 * metadata beneath — and four copies of it are how one of them ends up with a
 * hover highlight nobody asked for.
 *
 * A list is deliberately not a table. Rows here are read one at a time, and a
 * table's column rules earn nothing when no two rows are being compared.
 */

interface RecordListProps {
  /** What the list is called, for assistive technology. */
  readonly label: string;
  /** What would have been here, said in a sentence, when there is nothing. */
  readonly empty: string;
  readonly children: ReactNode;
  /** How many rows there are, so the empty state can be chosen here. */
  readonly count: number;
}

export function RecordList({ label, empty, count, children }: RecordListProps) {
  if (count === 0) {
    // An empty list always means "there is nothing". Every view is offered on
    // its own capability precisely so this can never mean "you may not see it".
    return <p className="record__empty">{empty}</p>;
  }
  return (
    <ul className="record" aria-label={label}>
      {children}
    </ul>
  );
}

interface RecordRowProps {
  /** The leading timestamp. Tabular figures, so a column of them lines up. */
  readonly at?: string;
  /** The row's primary line. */
  readonly primary: ReactNode;
  /** Secondary lines beneath it. */
  readonly meta?: ReactNode;
  /** Controls, when the row has any. Nothing in this panel navigates. */
  readonly actions?: ReactNode;
}

export function RecordRow({ at, primary, meta, actions }: RecordRowProps) {
  return (
    <li className="record__row">
      {at === undefined ? null : <span className="record__at">{at}</span>}
      <div className="record__text">
        <div className="record__primary">{primary}</div>
        {meta === undefined ? null : <div className="record__meta">{meta}</div>}
      </div>
      {actions === undefined ? null : <div className="record__actions">{actions}</div>}
    </li>
  );
}

/**
 * A person, drawn the way §15 requires.
 *
 * Their name if the directory knows one; otherwise the identifier, in
 * identifier style so a reader can tell at a glance that they are looking at an
 * identifier rather than at somebody named that. An action behind no
 * established identity reads "Not signed in", which is a fact, where an empty
 * cell would be an omission.
 */
export function PersonName({ label }: { readonly label: PersonLabel }) {
  switch (label.kind) {
    case 'named':
      return <span className="record__person">{label.name}</span>;
    case 'identifier':
      return <span className="record__identifier">{label.subject}</span>;
    default:
      return <span className="record__anonymous">Not signed in</span>;
  }
}
