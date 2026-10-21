import { useMemo, useState } from 'react';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import type { AppRoute } from '../routing/routes';
import { ResourceLink } from '../resources/ResourceLink';
import { Button } from '../ui/Button';
import { Pill } from '../ui/Pill';
import { stateLabels, type BotClient } from './model';
import { useBotRead } from './useBotRead';

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
        {current.error === undefined ? null : <p role="alert">{current.error}</p>}
        {page === undefined && current.error === undefined ? (
          <p role="status">Loading consumers</p>
        ) : null}
        {page === undefined ? null : (
          <>
            {page.items.length === 0 ? (
              <p>No consumers on this page.</p>
            ) : (
              <ul className="bots__rows" aria-label="Your consumers">
                {page.items.map((bot) => (
                  <li key={bot.bot_id}>
                    <ResourceLink
                      route={{ kind: 'bot', botId: bot.bot_id }}
                      onNavigate={onNavigate}
                    >
                      {bot.name}
                    </ResourceLink>
                    <Pill>{stateLabels[bot.state]}</Pill>
                    <span>Provider level {bot.channel?.level ?? 'not reported'}</span>
                    <span>
                      {bot.last_test === null
                        ? 'Not tested'
                        : `Last test accepted after ${bot.last_test.accepted_after_ms} ms`}
                    </span>
                  </li>
                ))}
              </ul>
            )}
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
          </>
        )}
      </div>
    </section>
  );
}
