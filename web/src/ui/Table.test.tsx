import { Fragment } from 'react';
import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Table, TableActions, TableCell, TableNote } from './Table';

/** The shared table — `docs/Web_Style.md` §12.6. */

const columns = [{ header: 'Name' }, { header: 'Count', align: 'end' as const }];

describe('the table', () => {
  it('names its columns and hides only the actions header visually', () => {
    render(
      <Table label="Channels" columns={columns} actions="Actions">
        <tr>
          <TableCell>Alpha</TableCell>
          <TableCell align="end">3</TableCell>
          <TableActions>
            <button type="button">Open</button>
          </TableActions>
        </tr>
      </Table>,
    );
    const table = screen.getByRole('table', { name: 'Channels' });
    const headers = within(table).getAllByRole('columnheader');
    expect(headers.map((header) => header.textContent)).toEqual(['Name', 'Count', 'Actions']);
    expect(within(headers[2]!).getByText('Actions')).toHaveClass('sr-only');
    expect(headers[1]).toHaveAttribute('data-align', 'end');
    const row = within(table).getAllByRole('row')[1]!;
    expect(within(row).getByRole('cell', { name: '3' })).toHaveAttribute('data-align', 'end');
    expect(within(row).getByRole('button', { name: 'Open' }).parentElement).toHaveClass(
      'table__actions',
    );
  });

  it('is a named, focusable region so a phone can scroll it without a pointer', () => {
    render(<Table label="Consumers" columns={columns} />);
    const region = screen.getByRole('region', { name: 'Consumers' });
    expect(region).toHaveAttribute('tabindex', '0');
    expect(region).toContainElement(screen.getByRole('table'));
  });

  it('reserves skeleton rows and names what is loading while there are no rows', () => {
    const { container } = render(
      <Table label="Sandboxes" columns={columns} actions="Actions" loading empty="None" />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('Loading Sandboxes');
    expect(screen.getByRole('table')).toHaveAttribute('aria-busy', 'true');
    const placeholders = container.querySelectorAll('tr.table__placeholder');
    expect(placeholders).toHaveLength(3);
    // One skeleton per column, the actions column included.
    expect(placeholders[0]?.querySelectorAll('td')).toHaveLength(3);
    expect(screen.queryByText('None')).toBeNull();
  });

  it('keeps the rows it has while a refresh loads, under its own loading label', () => {
    const { container } = render(
      <Table label="Activity" columns={columns} loading loadingLabel="Refreshing activity">
        <tr>
          <TableCell>Kept</TableCell>
          <TableCell>1</TableCell>
        </tr>
      </Table>,
    );
    expect(screen.getByText('Kept')).toBeInTheDocument();
    expect(container.querySelector('.table__placeholder')).toBeNull();
    expect(screen.getByRole('status')).toHaveTextContent('Refreshing activity');
  });

  it('says it is empty in one row across every column, headers kept', () => {
    render(
      <Table label="Channels" columns={columns} actions="Actions" empty="No channels yet">
        {[].map(() => null)}
      </Table>,
    );
    const cell = screen.getByRole('cell', { name: 'No channels yet' });
    expect(cell).toHaveAttribute('colspan', '3');
    expect(cell.closest('tr')).toHaveClass('table__empty');
    expect(screen.getAllByRole('columnheader')).toHaveLength(3);
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('draws nothing in the body when empty has nothing to say', () => {
    render(<Table label="Pools" columns={columns} />);
    expect(screen.getAllByRole('row')).toHaveLength(1);
  });

  it('places a note row beneath its record, spanning every column', () => {
    render(
      <Table label="Warm pools" columns={columns} actions="Actions">
        {['one', 'two'].map((name) => (
          <Fragment key={name}>
            <tr>
              <TableCell mono>{name}</TableCell>
              <TableCell>1</TableCell>
              <TableActions />
            </tr>
            {name === 'one' ? <TableNote>Waiting for the controller.</TableNote> : null}
          </Fragment>
        ))}
      </Table>,
    );
    const rows = screen.getAllByRole('row');
    expect(rows.map((row) => row.textContent)).toEqual([
      'NameCountActions',
      'one1',
      'Waiting for the controller.',
      'two1',
    ]);
    expect(rows[2]).toHaveClass('table__note');
    expect(within(rows[2]!).getByRole('cell')).toHaveAttribute('colspan', '3');
    expect(within(rows[1]!).getByRole('cell', { name: 'one' })).toHaveAttribute('data-mono');
  });
});
