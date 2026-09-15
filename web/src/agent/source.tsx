import type { AccessibleAgentSource } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';
import { useAgentFact, type AgentFactState } from './useAgentFact';
import { creationLabel, type ProvenanceState } from './provenance';

type SourceState = AgentFactState<AccessibleAgentSource>;

export function useAgentSource(agentId: string, enabled: boolean): SourceState {
  return useAgentFact(agentId, enabled, useCmaClient().readAgentSource);
}

export function AgentSourceLabel({
  source,
  provenance,
}: {
  readonly source: SourceState;
  readonly provenance: ProvenanceState;
}) {
  const trigger =
    source.agentId === provenance.agentId &&
    source.status === 'ready' &&
    source.value.access !== 'shared'
      ? source.value.trigger
      : undefined;
  return (
    <p className="agent-header__source">
      {creationLabel(provenance)}
      {trigger === undefined ? null : (
        <>
          {' · Published Trigger: '}
          <a href={`/t/${encodeURIComponent(trigger.public_id)}`}>{trigger.title}</a>
        </>
      )}
    </p>
  );
}
