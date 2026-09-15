import { useMemo, useState } from 'react';
import {
  ProtocolError,
  type BridgeChildCleanupStatus,
  type BridgeCleanupAllocation,
  type BridgeWorkspaceCleanupRequest,
} from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { sameCleanupTarget } from './useChildCleanup';
import { useWorkspaceCleanupRequest } from './useWorkspaceCleanupRequest';

interface Props {
  readonly client: BridgeViewsClient;
  readonly id: string;
  readonly selected?: BridgeCleanupAllocation;
  readonly child?: BridgeChildCleanupStatus;
  readonly revision?: number;
  readonly allowed: boolean;
  readonly onAccessChanged: () => void;
}
type Review = {
  readonly allocation: BridgeCleanupAllocation;
  readonly body: BridgeWorkspaceCleanupRequest;
};

/** Keep consent and uncertain intent mounted through parent authority and inventory refreshes. */
export function useWorkspaceCleanup({
  client,
  id,
  selected,
  child,
  revision,
  allowed,
  onAccessChanged,
}: Props) {
  const [inspected, setInspected] = useState<string>();
  const [review, setReview] = useState<Review>();
  const opened = selected !== undefined && inspected === selected.id;
  const read = useMemo(
    () =>
      !allowed || !opened || selected === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              const value = await client.bridges.readWorkspaceCleanup(id, selected.id, { signal });
              if (!sameCleanupTarget(selected, value))
                throw new ProtocolError('Allocation changed.');
              return { value };
            } catch {
              throw new Error(
                'Workspace cleanup or current access could not be confirmed. Refresh to check eligibility and the exact owned Workspace.',
              );
            }
          },
    [client, id, selected, allowed, opened],
  );
  const observation = useAuthorizedRead(read);
  const status = observation.state.status === 'ready' ? observation.state.value : undefined;
  const command = useWorkspaceCleanupRequest({
    client,
    id,
    allowed: allowed && status !== undefined && revision !== undefined,
    onAccepted: observation.reload,
    onDenied: () => {
      observation.reload();
      onAccessChanged();
    },
  });
  const supportedPlacement = selected?.placement === 'reuse_parent' && selected.state === 'retired';
  const childComplete =
    selected !== undefined &&
    child?.progress === 'complete' &&
    child.receipt !== null &&
    sameCleanupTarget(selected, child);
  const accepted = command.accepted?.allocation_id === selected?.id;
  const canRequest = Boolean(
    allowed &&
    supportedPlacement &&
    childComplete &&
    revision !== undefined &&
    status?.progress === 'not_requested' &&
    status.eligible &&
    status.runtime_supported &&
    status.workspace_revision !== null &&
    status.child_cleanup_receipt_id === child?.receipt?.id &&
    command.pending === undefined &&
    !command.busy &&
    !accepted,
  );
  const reviewCurrent = Boolean(
    review !== undefined &&
    canRequest &&
    status !== undefined &&
    sameCleanupTarget(review.allocation, status) &&
    review.body.expected_bridge_revision === revision &&
    review.body.expected_allocation_revision === status.allocation_revision &&
    review.body.expected_workspace_revision === status.workspace_revision &&
    review.body.child_cleanup_receipt_id === status.child_cleanup_receipt_id &&
    review.body.child_cleanup_receipt_id === child?.receipt?.id,
  );
  return {
    opened,
    observation,
    status,
    command,
    review,
    reviewCurrent,
    canRequest,
    childComplete,
    supportedPlacement,
    childReceiptMatches: status?.child_cleanup_receipt_id === child?.receipt?.id,
    canRecover: allowed && status !== undefined && revision !== undefined && !command.busy,
    acceptedWithoutObservation: accepted && status?.receipt === null,
    inspect: () => {
      if (allowed && childComplete && selected !== undefined) setInspected(selected.id);
    },
    cancelReview: () => setReview(undefined),
    reviewRemoval: () => {
      if (
        canRequest &&
        selected !== undefined &&
        status !== undefined &&
        revision !== undefined &&
        status.workspace_revision !== null &&
        status.child_cleanup_receipt_id !== null
      ) {
        setReview({
          allocation: { ...selected, target: { ...selected.target } },
          body: {
            expected_bridge_revision: revision,
            expected_allocation_revision: selected.revision,
            expected_workspace_revision: status.workspace_revision,
            child_cleanup_receipt_id: status.child_cleanup_receipt_id,
            action: 'remove_owned_workspace',
          },
        });
      }
    },
    confirm: () => {
      if (reviewCurrent && review !== undefined) {
        command.submit(review.allocation, review.body);
        setReview(undefined);
      }
    },
  };
}
export type WorkspaceCleanupController = ReturnType<typeof useWorkspaceCleanup>;
