import { useEffect, useState } from 'react';
import type { Cma, SandboxUsageEvents } from '@cma/client';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Table, TableCell } from '../ui/Table';
import { stateWord } from '../operator/records';
import { uptime, usageTime } from './format';

export function UsageHistory({
  client,
  id,
  onClose,
}: {
  readonly client: Pick<Cma['usage'], 'events'>;
  readonly id: string;
  readonly onClose: () => void;
}) {
  const [cursor, setCursor] = useState<string>();
  const [stored, setStored] = useState<{
    cursor?: string;
    page?: SandboxUsageEvents;
    error?: boolean;
  }>();
  useEffect(() => {
    const controller = new AbortController();
    void client.events(id, { cursor, limit: 50, signal: controller.signal }).then(
      (page) => {
        if (!controller.signal.aborted) setStored({ cursor, page });
      },
      () => {
        if (!controller.signal.aborted) setStored({ cursor, error: true });
      },
    );
    return () => controller.abort();
  }, [client, id, cursor]);
  const result = stored?.cursor === cursor ? stored : undefined;
  // The ledger identifies a transaction by Sandbox and revision. Distinct
  // checkpoints can have identical times and resource quantities.
  const transactions = new Map(result?.page?.items.map((event) => [event.transaction_id, event]));
  return (
    <section aria-label="Sandbox usage history">
      <h3>Lifecycle history</h3>
      <p>{id}</p>
      <Button onClick={onClose}>Close history</Button>
      {result?.error ? (
        <Alert tone="error" title="History unavailable">
          Close and reopen this history to try again.
        </Alert>
      ) : null}
      <Table
        label="Usage intervals"
        loading={!result}
        empty="No usage intervals recorded."
        columns={[
          { header: 'Event' },
          { header: 'Revision' },
          { header: 'Time (SGT)' },
          { header: 'Interval uptime' },
          { header: 'Interval CPU (millicores)' },
          { header: 'Interval RAM (MiB)' },
          { header: 'Interval disk (GiB)' },
        ]}
      >
        {[...transactions.values()].map((event) => (
          <tr key={event.transaction_id}>
            <TableCell>{stateWord(event.kind)}</TableCell>
            <TableCell>{event.revision}</TableCell>
            <TableCell>{usageTime(event.ended_at)}</TableCell>
            <TableCell>{uptime(event.quantity.uptime_seconds)}</TableCell>
            <TableCell>{event.allocation.cpu_millis}</TableCell>
            <TableCell>{event.allocation.memory_mib}</TableCell>
            <TableCell>{event.allocation.disk_gib}</TableCell>
          </tr>
        ))}
      </Table>
      {cursor ? <Button onClick={() => setCursor(undefined)}>First history page</Button> : null}
      {result?.page?.next ? (
        <Button onClick={() => setCursor(result.page?.next ?? undefined)}>Next history page</Button>
      ) : null}
    </section>
  );
}
