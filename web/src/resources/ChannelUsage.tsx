import { useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { ChannelHistory } from './ChannelHistory';

/** Category-only presentation; private binding/actor identifiers never enter this view. */
export function ChannelUsage({
  client,
  id,
}: {
  readonly client: ResourceViewsClient;
  readonly id: string;
}) {
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const value = await client.agents.channelUsage(id, { signal });
      return { value };
    },
    [client, id],
  );
  const [historyFor, setHistoryFor] = useState<{ read: typeof read }>();
  const current = useAuthorizedRead(read);
  const value = current.state.status === 'ready' ? current.state.value.channel_usage : undefined;
  const manager = current.state.status === 'ready' && current.state.value.bindings !== undefined;
  const [lastManagement, setLastManagement] = useState(false);
  if (current.state.status === 'ready' && lastManagement !== manager) setLastManagement(manager);
  const retainManagement = manager || (current.state.status === 'loading' && lastManagement);
  return (
    <>
      {current.state.status === 'loading' ? <p role="status">Checking channel activity</p> : null}
      {current.state.status === 'error' ? <p>Channel activity unavailable.</p> : null}
      {value === undefined ? null : (
        <p>
          {value.active
            ? 'Linked to a channel'
            : value.has_history
              ? 'Previously linked to a channel'
              : 'No channel activity recorded'}
          {value.last_used_at === null ? null : (
            <>
              {' '}
              · Last channel use{' '}
              <time dateTime={value.last_used_at}>
                {new Date(value.last_used_at).toLocaleString()}
              </time>
            </>
          )}
        </p>
      )}
      {manager && value?.has_history ? (
        <Button size="sm" disabled={current.refreshing} onClick={() => setHistoryFor({ read })}>
          Channel history
        </Button>
      ) : null}
      <Modal
        open={historyFor?.read === read}
        title="Channel history"
        onClose={() => setHistoryFor(undefined)}
      >
        {retainManagement ? (
          <ChannelHistory
            key={id}
            client={client}
            id={id}
            enabled={manager && !current.refreshing}
          />
        ) : (
          <p role="alert">Current access does not allow channel history.</p>
        )}
      </Modal>
    </>
  );
}
