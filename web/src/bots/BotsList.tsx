import { useMemo, useState } from 'react';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import type { AppRoute } from '../routing/routes';
import { ResourceLink } from '../resources/ResourceLink';
import { Alert } from '../ui/Alert';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Table, TableCell } from '../ui/Table';
import { stateLabels, stateTones, type BotClient } from './model';
import { useBotRead } from './useBotRead';

const COLUMNS = [
  { header: 'Name' },
  { header: 'State' },
  { header: 'Provider level' },
  { header: 'Last test' },
  { header: 'Updated' },
];

export function BotsList({
  client,
  onNavigate,
}: {
  readonly client: BotClient;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => (signal: AbortSignal) => client.bots.list({ after: cursor, limit: 50, signal }),
    [client, cursor],
  );
  const current = useBotRead(read);
  const page = current.value;
  useDocumentTitle('Consumers');
  return (
    <section aria-labelledby="bots-title">
      <div className="bots__header">
        <PageHeader
          breadcrumb={[{ label: 'Event Gateway' }, { label: 'Consumers' }]}
          title="Consumers"
          headingId="bots-title"
          description="Choose an Event Gateway connection and configure the Agent that handles its messages."
          actions={
            <>
              <Button onClick={current.reload}>Refresh consumers</Button>
              <Button variant="primary" onClick={() => onNavigate({ kind: 'bot_new' })}>
                New consumer
              </Button>
            </>
          }
        />
      </div>
      <div className="bots__body">
        {current.error === undefined ? null : <Alert tone="error">{current.error}</Alert>}
        <Table
          label="Your consumers"
          placement="bots__table"
          columns={COLUMNS}
          loading={page === undefined && current.error === undefined}
          loadingLabel="Loading consumers"
          empty={
            page === undefined
              ? undefined
              : cursors.length === 1
                ? 'No consumers yet'
                : 'No consumers on this page.'
          }
        >
          {page?.items.map((bot) => (
            <tr key={bot.bot_id}>
              <TableCell>
                <ResourceLink route={{ kind: 'bot', botId: bot.bot_id }} onNavigate={onNavigate}>
                  {bot.name}
                </ResourceLink>
              </TableCell>
              <TableCell>
                <Badge tone={stateTones[bot.state]}>{stateLabels[bot.state]}</Badge>
              </TableCell>
              <TableCell>{bot.channel?.level ?? 'Not reported'}</TableCell>
              <TableCell>
                {bot.last_test === null
                  ? 'Not tested'
                  : `Accepted after ${String(bot.last_test.accepted_after_ms)} ms`}
              </TableCell>
              <TableCell>
                <time dateTime={bot.updated_at}>{new Date(bot.updated_at).toLocaleString()}</time>
              </TableCell>
            </tr>
          ))}
        </Table>
        {page === undefined ? null : (
          <nav className="bots__actions" aria-label="Consumer pages">
            <Button
              disabled={cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous page
            </Button>
            <span>Page {cursors.length}</span>
            <Button
              disabled={
                !current.fresh || page.next_after === null || cursors.includes(page.next_after)
              }
              onClick={() => setCursors((values) => [...values, page.next_after ?? undefined])}
            >
              Next page
            </Button>
          </nav>
        )}
      </div>
    </section>
  );
}
