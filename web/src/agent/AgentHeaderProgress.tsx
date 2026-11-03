import type { AgentResourceState } from './controller';
import { PlanProgress } from './PlanProgress';
import { TurnCompletion } from './TurnCompletion';

export function AgentHeaderProgress({
  state,
  starting,
}: {
  readonly state: Extract<AgentResourceState, { status: 'ready' }>;
  readonly starting: boolean;
}) {
  const { conversation } = state;
  const withheld =
    state.historyLoading ||
    state.historyUnavailable ||
    starting ||
    ['stopped', 'failed', 'deleted'].includes(state.detail.status);
  return (
    <>
      <PlanProgress plans={conversation.plans} activeRunId={conversation.activeRunId} />
      {withheld ? null : <TurnCompletion conversation={conversation} />}
    </>
  );
}
