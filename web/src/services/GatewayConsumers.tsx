import { useEffect, useState } from 'react';
import type { Bot } from '@chronoai/cma-sdk';
import type { BotClient } from '../bots/model';
import type { AppRoute } from '../routing/routes';
import { TreeRow } from '../shell/TreeRow';
import { keyboardNavigation } from '../shell/treePresentation';
import { IconRefreshOutline16 } from '../ui/icons';
import '../shell/Tree.css';
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
        <button
          type="button"
          className="navigation-filters__toggle"
          disabled={busy}
          onClick={refresh}
          aria-label="Refresh consumers"
          title="Refresh consumers"
        >
          <IconRefreshOutline16 />
        </button>
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
      <div
        className="agent-navigation"
        role="tree"
        aria-label="Consumers"
        onKeyDown={keyboardNavigation}
      >
        {items.map((item) => (
          <TreeRow
            key={item.bot_id}
            nodeId={`consumer:${item.bot_id}`}
            level={1}
            layout="line"
            label={item.name}
            icon={null}
            selected={selected === item.bot_id}
            onNavigate={onNavigate}
            onActivate={() => onNavigate({ kind: 'bot', botId: item.bot_id })}
            meta={<GatewayRecordMeta compact createdAt={item.created_at} owner={item.owner} />}
          />
        ))}
      </div>
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
