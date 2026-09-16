import { useMemo, useState } from 'react';
import type { BridgePartition } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Button } from '../ui/Button';
import { PartitionSummary, PartitionActivity } from './PartitionStatus';
import { usePartitionCommand } from './usePartitionCommand';
import { PartitionCommands } from './PartitionCommands';

interface Props {
  readonly client: BridgeViewsClient;
  readonly viewerId: string;
  readonly id: string;
  readonly ownerScope?: string;
  readonly revision?: number;
  readonly allowed: boolean;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged?: () => void;
}

export function BridgePartitions(props: Props) {
  const [scope, setScope] = useState({
    client: props.client,
    viewer: props.viewerId,
    id: props.id,
    owner: props.ownerScope,
    generation: 0,
  });
  const owner = props.ownerScope ?? scope.owner;
  if (
    scope.client !== props.client ||
    scope.viewer !== props.viewerId ||
    scope.id !== props.id ||
    scope.owner !== owner
  ) {
    setScope({
      client: props.client,
      viewer: props.viewerId,
      id: props.id,
      owner,
      generation: scope.generation + 1,
    });
  }
  return <PartitionBrowser key={scope.generation} {...props} />;
}

function PartitionBrowser({ client, id, revision, allowed, onNavigate, onAccessChanged }: Props) {
  const [opened, setOpened] = useState(false);
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const [selected, setSelected] = useState<string>();
  const cursor = cursors.at(-1);
  const read = useMemo(
    () =>
      !allowed || !opened || revision === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              return {
                value: await client.bridges.listPartitions(id, { cursor, limit: 25, signal }),
              };
            } catch {
              throw new Error('Partition status is unavailable. Refresh to check current access.');
            }
          },
    [client, id, revision, allowed, opened, cursor],
  );
  const inventory = useAuthorizedRead(read);
  const page = inventory.state.status === 'ready' ? inventory.state.value : undefined;
  const readDetail = useMemo(
    () =>
      !allowed || selected === undefined || revision === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              return { value: await client.bridges.readPartition(id, selected, { signal }) };
            } catch {
              throw new Error(
                'This partition is unavailable. Refresh to check current access and status.',
              );
            }
          },
    [client, id, revision, selected, allowed],
  );
  const observation = useAuthorizedRead(readDetail);
  const detail = observation.state.status === 'ready' ? observation.state.value : undefined;
  const command = usePartitionCommand({
    client,
    id,
    allowed: allowed && revision !== undefined,
    onAccepted: () => {
      inventory.reload();
      observation.reload();
    },
    onDenied: () => {
      inventory.reload();
      observation.reload();
      onAccessChanged?.();
    },
  });
  const locked = command.busy || command.pending !== undefined;
  if (!allowed) return null;
  function inspect(partition: BridgePartition) {
    setSelected(partition.id);
  }
  return (
    <section className="bridges__fields" aria-labelledby="bridge-partitions-title">
      <h2 id="bridge-partitions-title">Partitions and conversations</h2>
      <p>Find the conversation and accepted Channel work for each retained partition.</p>
      {!opened ? (
        <Button onClick={() => setOpened(true)}>Show partitions</Button>
      ) : (
        <>
          <Button
            onClick={() => {
              inventory.reload();
              observation.reload();
            }}
          >
            Refresh partitions
          </Button>
          {inventory.state.status === 'loading' ? (
            <p role="status">Checking partitions and current access</p>
          ) : null}
          {inventory.state.status === 'error' ? (
            <p role="alert">{inventory.state.message}</p>
          ) : null}
          {page === undefined ? null : (
            <>
              {page.items.length === 0 ? (
                <p>No partitions on this page.</p>
              ) : (
                <ul className="bridges__rows" aria-label="Bridge partitions">
                  {page.items.map((partition) => (
                    <li key={partition.id}>
                      <PartitionSummary partition={partition} onNavigate={onNavigate} />
                      <Button disabled={locked} onClick={() => inspect(partition)}>
                        Inspect generation {partition.generation}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
              <div className="bridges__actions">
                <Button
                  disabled={locked || cursors.length === 1}
                  onClick={() => {
                    setSelected(undefined);
                    setCursors((values) => values.slice(0, -1));
                  }}
                >
                  Previous partitions
                </Button>
                <Button
                  disabled={locked || page.next_cursor === null}
                  onClick={() => {
                    if (page.next_cursor !== null) {
                      setSelected(undefined);
                      setCursors((values) => [...values, page.next_cursor!]);
                    }
                  }}
                >
                  Next partitions
                </Button>
              </div>
            </>
          )}
          {selected === undefined ? null : (
            <section className="bridges__fields" aria-label="Selected partition status">
              <h3>Selected partition</h3>
              {observation.state.status === 'loading' ? (
                <p role="status">Checking selected partition and conversation access</p>
              ) : null}
              {observation.state.status === 'error' ? (
                <p role="alert">{observation.state.message}</p>
              ) : null}
              {detail === undefined ? null : (
                <>
                  <PartitionSummary partition={detail.partition} onNavigate={onNavigate} />
                  <PartitionActivity activity={detail.activity} />
                </>
              )}
              <PartitionCommands
                partition={detail?.partition}
                revision={revision}
                allowed={allowed}
                command={command}
              />
              <Button onClick={observation.reload}>Refresh selected partition</Button>
            </section>
          )}
        </>
      )}
    </section>
  );
}
