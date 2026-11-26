import { within } from '@testing-library/react';

/** The cell of `row` under the column headed `header`. */
export function cell(row: HTMLElement, header: string) {
  const table = row.closest('table')!;
  const index = within(table)
    .getAllByRole('columnheader')
    .findIndex((column) => column.textContent === header);
  return within(row).getAllByRole('cell')[index]!;
}
