import { useMemo, useState } from 'react';
import { ProtocolError, type BridgeCleanupAllocation } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { ResourceLink } from '../resources/ResourceLink';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Button } from '../ui/Button';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { ChildCleanupStatus } from './ChildCleanupStatus';
import { sameCleanupTarget, useChildCleanup } from './useChildCleanup';

interface Props {
  readonly client: BridgeViewsClient;
  readonly viewerId: string;
  readonly id: string;
  readonly ownerScope?: string;
  readonly revision?: number;
  readonly allowed: boolean;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged: () => void;
}

export function BridgeAllocations(props: Props) {
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
  return <AllocationBrowser key={scope.generation} {...props} />;
}

function AllocationBrowser({ client, id, revision, allowed, onNavigate, onAccessChanged }: Props) {
  const [opened, setOpened] = useState(false);
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const [selected, setSelected] = useState<BridgeCleanupAllocation>();
  const [review, setReview] = useState<{ allocation: BridgeCleanupAllocation; revision: number }>();
  const cursor = cursors.at(-1);
  const listRead = useMemo(
    () =>
      !allowed || !opened
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              return {
                value: await client.bridges.listAllocations(id, { cursor, limit: 25, signal }),
              };
            } catch {
              throw new Error(
                'Allocation access could not be checked. Refresh to check current access.',
              );
            }
          },
    [client, id, allowed, opened, cursor],
  );
  const inventory = useAuthorizedRead(listRead);
  const page = inventory.state.status === 'ready' ? inventory.state.value : undefined;
  const statusRead = useMemo(
    () =>
      !allowed || selected === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              const value = await client.bridges.readChildCleanup(id, selected.id, { signal });
              if (!sameCleanupTarget(selected, value))
                throw new ProtocolError('Allocation changed.');
              return { value };
            } catch {
              throw new Error(
                'Cleanup access or the selected allocation could not be confirmed. Refresh allocations and review the exact child again.',
              );
            }
          },
    [client, id, allowed, selected],
  );
  const observation = useAuthorizedRead(statusRead);
  const status = observation.state.status === 'ready' ? observation.state.value : undefined;
  const command = useChildCleanup({
    client,
    id,
    allowed: allowed && revision !== undefined && status !== undefined,
    onAccepted: observation.reload,
    onDenied: () => {
      observation.reload();
      onAccessChanged();
    },
  });
  const retained = command.pending !== undefined;
  const alreadyAccepted = command.accepted?.allocation_id === selected?.id;
  const canRequest =
    status?.progress === 'not_requested' &&
    selected?.placement === 'reuse_parent' &&
    selected.state === 'retired' &&
    revision !== undefined &&
    !retained &&
    !alreadyAccepted;
  const reviewCurrent =
    review !== undefined &&
    revision === review.revision &&
    status !== undefined &&
    sameCleanupTarget(review.allocation, status) &&
    canRequest;
  if (!allowed) return null;
  return (
    <section className="bridges__fields" aria-labelledby="bridge-allocations-title">
      <h2 id="bridge-allocations-title">Allocations and child cleanup</h2>
      <p>
        Inspect the children retained by this Bridge. Cleanup removes an explicitly selected retired
        Agent and Worktree while preserving its Workspace, sibling Agents and history.
      </p>
      {!opened ? (
        <Button onClick={() => setOpened(true)}>Show allocations</Button>
      ) : (
        <>
          <Button disabled={command.busy} onClick={inventory.reload}>
            Refresh allocations
          </Button>
          {inventory.state.status === 'loading' ? (
            <p role="status">Checking allocations and current parent access</p>
          ) : null}
          {inventory.state.status === 'error' ? (
            <p role="alert">{inventory.state.message}</p>
          ) : null}
          {page === undefined ? null : (
            <>
              {page.items.length === 0 ? (
                <p>
                  {page.next_cursor === null
                    ? 'No accessible allocations on this page.'
                    : 'No accessible allocations on this page. Continue to the next page.'}
                </p>
              ) : (
                <ul className="bridges__rows" aria-label="Bridge allocations">
                  {page.items.map((allocation) => (
                    <li key={allocation.id}>
                      <div>
                        <strong>Generation {allocation.generation}</strong>
                        <p>{allocation.id}</p>
                      </div>
                      <span>
                        {allocation.state.replaceAll('_', ' ')} ·{' '}
                        {allocation.placement.replaceAll('_', ' ')}
                      </span>
                      <AllocationLinks allocation={allocation} onNavigate={onNavigate} />
                      <Button
                        disabled={retained || command.busy}
                        onClick={() => {
                          setSelected(allocation);
                          setReview(undefined);
                        }}
                      >
                        Inspect cleanup for generation {allocation.generation}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
              <div className="bridges__actions">
                <Button
                  disabled={command.busy || cursors.length === 1}
                  onClick={() => setCursors((values) => values.slice(0, -1))}
                >
                  Previous allocations
                </Button>
                <Button
                  disabled={command.busy || page.next_cursor === null}
                  onClick={() => {
                    if (page.next_cursor !== null)
                      setCursors((values) => [...values, page.next_cursor!]);
                  }}
                >
                  Next allocations
                </Button>
              </div>
            </>
          )}
          {selected === undefined ? null : (
            <section className="bridges__fields" aria-label="Selected child cleanup">
              <h3>Selected child cleanup</h3>
              {observation.state.status === 'loading' ? (
                <p role="status">Checking selected child access and cleanup status</p>
              ) : null}
              {observation.state.status === 'error' ? (
                <p role="alert">{observation.state.message}</p>
              ) : null}
              {status === undefined ? null : (
                <>
                  <p>
                    Generation {selected.generation} · Allocation revision{' '}
                    {status.allocation_revision}
                  </p>
                  <AllocationLinks allocation={selected} onNavigate={onNavigate} />
                  <ChildCleanupStatus status={status} />
                  {alreadyAccepted && status.receipt === null ? (
                    <p role="status">
                      Cleanup was accepted. Refresh to recover its current progress before any
                      further action.
                    </p>
                  ) : null}
                  {selected.placement === 'reuse_parent' && selected.state === 'retired' ? (
                    <Button
                      variant="danger"
                      disabled={!canRequest || command.busy}
                      onClick={() => {
                        if (revision !== undefined) setReview({ allocation: selected, revision });
                      }}
                    >
                      Remove retired child
                    </Button>
                  ) : (
                    <p>
                      Only a retired child created in a shared parent supports this cleanup action.
                    </p>
                  )}
                </>
              )}
              <div className="bridges__actions">
                <Button disabled={command.busy} onClick={observation.reload}>
                  Refresh cleanup status
                </Button>
              </div>
              {command.busy ? (
                <p role="status">Checking acceptance of the original cleanup request</p>
              ) : null}
              {command.message === undefined ? null : <p role="status">{command.message}</p>}
              {!retained ? null : (
                <Button
                  disabled={command.busy || status === undefined || revision === undefined}
                  onClick={command.recover}
                >
                  Recover original cleanup request
                </Button>
              )}
            </section>
          )}
        </>
      )}
      <RiskConfirmation
        open={review !== undefined}
        title="Remove this retired child?"
        description={
          review === undefined
            ? ''
            : `Remove Agent ${review.allocation.target.agent_id} and Worktree ${review.allocation.target.worktree_id} from generation ${review.allocation.generation}. Their files will be removed. The Workspace, sibling Agents and retained history stay in place. Accepted commands and replies must drain before removal; an unknown outcome will hold cleanup.`
        }
        acknowledgeLabel="I understand that this child's Agent home and Worktree files will be removed."
        confirmLabel="Request child removal"
        busy={command.busy}
        confirmDisabled={!reviewCurrent}
        status={
          review !== undefined && !reviewCurrent
            ? 'Current access or revisions changed. Cancel and refresh before reviewing this child again.'
            : undefined
        }
        onCancel={() => setReview(undefined)}
        onConfirm={() => {
          if (reviewCurrent) {
            command.submit(review.allocation, review.revision);
            setReview(undefined);
          }
        }}
      />
    </section>
  );
}

function AllocationLinks({
  allocation,
  onNavigate,
}: {
  readonly allocation: BridgeCleanupAllocation;
  readonly onNavigate: Props['onNavigate'];
}) {
  return (
    <div className="bridges__allocation-target">
      <ResourceLink
        route={{ kind: 'sandbox', sandboxId: allocation.target.sandbox_id }}
        onNavigate={onNavigate}
      >
        Sandbox {allocation.target.sandbox_id}
      </ResourceLink>
      <ResourceLink
        route={{ kind: 'workspace', workspaceId: allocation.target.workspace_id }}
        onNavigate={onNavigate}
      >
        Workspace {allocation.target.workspace_id}
      </ResourceLink>
      <ResourceLink
        route={{ kind: 'agent', agentId: allocation.target.agent_id }}
        onNavigate={onNavigate}
      >
        Agent {allocation.target.agent_id}
      </ResourceLink>
      <span>Worktree {allocation.target.worktree_id}</span>
    </div>
  );
}
