import { useCallback } from 'react';
import { useCmaClient } from '../api/context';
import { UnavailableView } from '../ConsoleViews';
import type { AppRoute } from '../routing/routes';
import { AgentShareAccept } from './AgentShareAccept';

/** Completes a link handoff by opening the Agent in the recipient's shared scope. */
export function AgentSharePage({
  shareId,
  onScope,
  onRefresh,
  onNavigate,
}: {
  readonly shareId: string;
  readonly onScope: (scope: string) => void;
  readonly onRefresh: () => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const client = useCmaClient().resourceViews;
  const accepted = useCallback(
    (agentId: string) => {
      onScope('shared');
      onRefresh();
      onNavigate({ kind: 'agent', agentId });
    },
    [onScope, onRefresh, onNavigate],
  );
  return client === undefined ? (
    <UnavailableView
      title="Agent sharing unavailable"
      reason="This CMA deployment does not support shared Agent links. Ask the person who shared it to send the conversation another way."
    />
  ) : (
    <AgentShareAccept client={client} shareId={shareId} onAccepted={accepted} />
  );
}
