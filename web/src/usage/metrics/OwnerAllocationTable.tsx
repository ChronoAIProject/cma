import { useState } from 'react';
import type { UsageOwnerAllocation } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
import { Table, TableCell } from '../../ui/Table';
import { displayQuantity } from './model';
import { ownerAllocationText } from './OwnerAllocationName';

/** Keyboard-readable values use bounded pages even when All has thousands of owners. */
export function OwnerAllocationTable({
  items,
  size,
  unit,
}: {
  readonly items: readonly UsageOwnerAllocation[];
  readonly size: number;
  readonly unit: string;
}) {
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState(0);
  const page = Math.min(selected, Math.max(0, Math.ceil(size / 20) - 1));
  const rows = items.slice(page * 20, Math.min(size, (page + 1) * 20));
  return (
    <details className="usage-chart-data" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>View owner allocation chart data</summary>
      {open && (
        <>
          <Table
            label="Owner allocation chart data"
            placement="usage-chart-table"
            columns={[
              { header: 'Owner' },
              { header: `Allocation (${unit})`, align: 'end' },
              { header: 'Sandboxes', align: 'end' },
              { header: 'History' },
            ]}
          >
            {rows.map((row) => (
              <tr key={row.subject}>
                <TableCell>
                  <span title={ownerAllocationText(row)}>{row.label}</span>
                  {row.disambiguator && <small> · {row.disambiguator}</small>}
                </TableCell>
                <TableCell align="end">{displayQuantity(row.allocated_hours)}</TableCell>
                <TableCell align="end">{displayQuantity(row.sandbox_count)}</TableCell>
                <TableCell>{row.complete ? 'Complete' : 'Partial'}</TableCell>
              </tr>
            ))}
          </Table>
          <div className="usage-metric-actions">
            <span>
              Rows {page * 20 + 1}–{page * 20 + rows.length} of {size.toLocaleString()} shown owners
            </span>
            <Button
              variant="outline"
              size="sm"
              disabled={page === 0}
              onClick={() => setSelected(page - 1)}
            >
              Previous owner values
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={(page + 1) * 20 >= size}
              onClick={() => setSelected(page + 1)}
            >
              Next owner values
            </Button>
          </div>
        </>
      )}
    </details>
  );
}
