import { useMemo, useState } from 'react';
import {
  ApiError,
  type BridgeDelivery,
  type BridgeDeliveryRecoveryAction,
} from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { Button } from '../ui/Button';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { bridgeFailure, useBridgeCommand } from './commands';
import { DeliveryHistory, deliveryStatus } from './DeliveryHistory';

export function BridgeDeliveries({
  client,
  id,
  revision,
  allowed,
  onAccessChanged,
}: {
  readonly client: BridgeViewsClient;
  readonly id: string;
  readonly revision: number;
  readonly allowed: boolean;
  readonly onAccessChanged: () => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const [notice, setNotice] = useState<string>();
  const cursor = cursors.at(-1);
  const read = useMemo(
    () =>
      !allowed
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              return {
                value: await client.bridges.listDeliveries(id, { cursor, limit: 25, signal }),
              };
            } catch (error) {
              if (
                !signal.aborted &&
                error instanceof ApiError &&
                [401, 403, 404].includes(error.status)
              )
                onAccessChanged();
              throw new Error(bridgeFailure(error));
            }
          },
    [client, id, cursor, allowed, onAccessChanged],
  );
  const current = useAuthorizedRead(read);
  const page = current.state.status === 'ready' ? current.state.value : undefined;
  // Keep an uncertain command's exact body/key above list refreshes and paging.
  const command = useBridgeCommand(allowed && page !== undefined);
  const recover = (item: BridgeDelivery, action: BridgeDeliveryRecoveryAction) => {
    const body = {
      item_id: item.id,
      expected_revision: item.revision,
      expected_bridge_revision: revision,
      action,
    };
    command.run(
      action.kind === 'retry'
        ? 'Queue original reply'
        : action.kind === 'abandon'
          ? 'Abandon reply'
          : 'Refresh delivery state',
      async (options) => {
        try {
          const receipt = await client.bridges.recoverDelivery(id, body, options);
          options.signal?.throwIfAborted();
          setNotice(
            action.kind === 'retry'
              ? 'The original reply is queued again. Delivery has not been confirmed.'
              : action.kind === 'abandon'
                ? 'Further attempts for this reply were abandoned. Earlier delivery may still have occurred.'
                : `Delivery state checked: ${deliveryStatus[receipt.result.status]}.`,
          );
          current.reload();
        } catch (error) {
          if (error instanceof ApiError && [401, 403, 404].includes(error.status))
            onAccessChanged();
          throw error;
        }
      },
    );
  };
  if (!allowed) return null;
  return (
    <section className="bridges__fields" aria-labelledby="bridge-deliveries-title">
      <h2 id="bridge-deliveries-title">Channel replies</h2>
      <p>
        Replies use the original conversation and Bridge settings. Queued replies have not been
        confirmed delivered.
      </p>
      <Button onClick={current.reload} disabled={command.busy}>
        Refresh replies
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">Checking reply history and current management access</p>
      ) : null}
      {current.state.status === 'error' ? <p role="alert">{current.state.message}</p> : null}
      {page === undefined ? null : (
        <>
          {page.items.length === 0 ? (
            <p>No channel replies on this page.</p>
          ) : (
            <ul aria-label="Channel reply history">
              {page.items.map((item) => (
                <DeliveryHistory
                  key={`${item.id}:${item.revision}`}
                  item={item}
                  disabled={command.locked}
                  onRecover={(action) => recover(item, action)}
                />
              ))}
            </ul>
          )}
          <div className="bridges__actions">
            <Button
              disabled={command.locked || cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous replies
            </Button>
            <Button
              disabled={command.locked || page.next_cursor === null}
              onClick={() => {
                if (page.next_cursor !== null)
                  setCursors((values) => [...values, page.next_cursor!]);
              }}
            >
              Next replies
            </Button>
          </div>
        </>
      )}
      {allowed && notice !== undefined ? <p role="status">{notice}</p> : null}
      {command.error === undefined ? null : <p role="alert">{command.error}</p>}
      {command.retryLabel === undefined || command.busy ? null : (
        <Button disabled={page === undefined} onClick={command.retry}>
          Recover previous action
        </Button>
      )}
    </section>
  );
}
