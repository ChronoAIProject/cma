import { Children, createContext, useContext, type ReactNode } from 'react';
import { Skeleton } from './Skeleton';
import { Button } from './Button';
import './Table.css';

/** One column's header. `end` aligns a count or the actions to the right edge. */
export interface TableColumn {
  readonly header: string;
  readonly align?: 'end';
  readonly sort?: 'ascending' | 'descending';
  readonly onSort?: () => void;
}

const SpanContext = createContext(1);
const PLACEHOLDER_ROWS = 3;

/**
 * Records somebody compares across rows — `docs/Web_Style.md` §12.6.
 *
 * A real table, so a screen reader announces which column a fact belongs to
 * instead of leaving every reader to rediscover it in a list of cards. Rows
 * are the caller's own `<tr>` elements: a row that keeps state, such as a
 * warm pool's draft target, stays one component rather than being split into
 * per-cell render callbacks that cannot share it.
 *
 * The table owns the states every list repeats — loading reserves the rows'
 * geometry, empty keeps the headers — so no surface reinvents them.
 */
export function Table({
  label,
  columns,
  actions,
  loading = false,
  loadingLabel,
  empty,
  placement,
  children,
}: {
  /** Names both the table and its scrolling region. */
  readonly label: string;
  readonly columns: readonly TableColumn[];
  /** The visually hidden header of the trailing actions column, when rows have actions. */
  readonly actions?: string;
  /** Skeleton rows stand in only while there are no rows to keep. */
  readonly loading?: boolean;
  readonly loadingLabel?: string;
  /** Said in one full-width row when there is nothing to list and nothing loading. */
  readonly empty?: ReactNode;
  /** Extra class for the owning surface's placement only. */
  readonly placement?: string;
  readonly children?: ReactNode;
}) {
  const span = columns.length + (actions === undefined ? 0 : 1);
  const rows = Children.toArray(children);
  const placeholder = loading && rows.length === 0;
  return (
    <div
      className={['table', placement ?? ''].filter(Boolean).join(' ')}
      role="region"
      aria-label={label}
      // The box scrolls sideways on a narrow screen; a keyboard must reach it.
      tabIndex={0}
    >
      <table className="table__grid" aria-label={label} aria-busy={loading || undefined}>
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column.header} scope="col" data-align={column.align} aria-sort={column.sort}>
                {column.onSort ? (
                  <Button
                    size="sm"
                    onClick={column.onSort}
                    aria-label={`Sort by ${column.header} ${column.sort === 'ascending' ? 'descending' : 'ascending'}`}
                  >
                    {column.header}
                    <span aria-hidden="true">
                      {column.sort === 'ascending'
                        ? ' ↑'
                        : column.sort === 'descending'
                          ? ' ↓'
                          : ''}
                    </span>
                  </Button>
                ) : (
                  column.header
                )}
              </th>
            ))}
            {actions === undefined ? null : (
              <th scope="col" data-align="end">
                <span className="sr-only">{actions}</span>
              </th>
            )}
          </tr>
        </thead>
        <tbody>
          <SpanContext.Provider value={span}>
            {placeholder ? (
              Array.from({ length: PLACEHOLDER_ROWS }, (_, row) => (
                <tr key={row} className="table__placeholder" aria-hidden="true">
                  {Array.from({ length: span }, (_, cell) => (
                    <td key={cell}>
                      <Skeleton width={`${String(70 - (((row * 3 + cell) * 11) % 35))}%`} />
                    </td>
                  ))}
                </tr>
              ))
            ) : rows.length === 0 && empty !== undefined ? (
              <TableNote empty>{empty}</TableNote>
            ) : (
              rows
            )}
          </SpanContext.Provider>
        </tbody>
      </table>
      {loading ? (
        <p className="sr-only" role="status">
          {loadingLabel ?? `Loading ${label}`}
        </p>
      ) : null}
    </div>
  );
}

/** A cell; `mono` sets an identifier or slug in the code face. */
export function TableCell({
  children,
  mono = false,
  align,
}: {
  readonly children?: ReactNode;
  readonly mono?: boolean;
  readonly align?: 'end';
}) {
  return (
    <td data-mono={mono || undefined} data-align={align}>
      {children}
    </td>
  );
}

/** The trailing, right-aligned cell holding a row's compact actions. */
export function TableActions({ children }: { readonly children?: ReactNode }) {
  return (
    <td data-align="end">
      <div className="table__actions">{children}</div>
    </td>
  );
}

/**
 * A row's own note — a failure, an unconfirmed outcome — spanning every
 * column directly beneath it, with no divider, so it reads as part of that row.
 */
export function TableNote({
  children,
  empty = false,
}: {
  readonly children?: ReactNode;
  readonly empty?: boolean;
}) {
  const span = useContext(SpanContext);
  return (
    <tr className={empty ? 'table__empty' : 'table__note'}>
      <td colSpan={span}>{children}</td>
    </tr>
  );
}
