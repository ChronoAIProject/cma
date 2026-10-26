import { useEffect, useState } from 'react';
import type { Bot } from '@chronoai/cma-sdk';
import type { BotClient } from '../bots/model';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { Alert } from '../ui/Alert';
import { GatewayRecordMeta } from './GatewayRecordMeta';

export function GatewayConsumers({
  client,
  selected,
  onNavigate,
}: {
  readonly client: BotClient;
  readonly selected?: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [items, setItems] = useState<readonly Bot[]>([]);
  const [cursor, setCursor] = useState<string>();
  const [next, setNext] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(true);
  useEffect(() => {
    const controller = new AbortController();
    void client.bots
      .list({ after: cursor, limit: 50, signal: controller.signal })
      .then((page) => {
        if (controller.signal.aborted) return;
        setItems((previous) =>
          cursor
            ? [...new Map([...previous, ...page.items].map((item) => [item.bot_id, item])).values()]
            : page.items,
        );
        setNext(page.next_after ?? undefined);
        setError(undefined);
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted)
          setError(cause instanceof Error ? cause.message : 'Consumers could not be loaded.');
      })
      .finally(() => {
        if (!controller.signal.aborted) setBusy(false);
      });
    return () => controller.abort();
  }, [client, cursor, revision]);
  const refresh = () => {
    setBusy(true);
    setCursor(undefined);
    setRevision((value) => value + 1);
  };
  return (
    <section className="gateway-sidebar" aria-label="Consumers">
      <div className="gateway-sidebar__heading">
        <Button size="sm" onClick={() => onNavigate({ kind: 'bot_new' })}>
          New consumer
        </Button>
        <Button size="sm" onClick={refresh} disabled={busy} aria-label="Refresh consumers">
          Refresh
        </Button>
      </div>
      {error ? (
        <Alert tone="error" actions={<Button onClick={refresh}>Try again</Button>}>
          {error}
        </Alert>
      ) : null}
      {busy && items.length === 0 ? <p role="status">Loading consumers…</p> : null}
      {!busy && !error && items.length === 0 ? (
        <p className="gateway-sidebar__empty">No consumers yet.</p>
      ) : null}
      <ul className="gateway-sidebar__list">
        {items.map((item) => (
          <li key={item.bot_id}>
            <button
              type="button"
              className="gateway-sidebar__channel"
              aria-current={selected === item.bot_id ? 'page' : undefined}
              onClick={() => onNavigate({ kind: 'bot', botId: item.bot_id })}
            >
              <span>{item.name}</span>
              <GatewayRecordMeta createdAt={item.created_at} owner={item.owner} />
            </button>
          </li>
        ))}
      </ul>
      {next ? (
        <Button
          disabled={busy}
          onClick={() => {
            setBusy(true);
            setCursor(next);
          }}
        >
          Load more consumers
        </Button>
      ) : null}
    </section>
  );
}
