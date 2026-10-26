import type { AccessibleAgentSource } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';
import { useAgentFact, type AgentFactState } from './useAgentFact';
import type { MenuEntry } from '../ui/Menu';
import { creationLabel, type ProvenanceState } from './provenance';

type SourceState = AgentFactState<AccessibleAgentSource>;

export function useAgentSource(agentId: string, enabled: boolean): SourceState {
  return useAgentFact(agentId, enabled, useCmaClient().readAgentSource);
}

/**
 * The creation origin as rows of the Agent header's menu — `docs/Web_Style.md` §13.
 *
 * Creation is a heading row: it is a fact, not something to choose. A Trigger
 * published from this Agent is a separate link row, so later reuse of that
 * Trigger can never be read as how the Agent was created.
 */
export function agentOriginEntries(
  source: SourceState,
  provenance: ProvenanceState,
  originLabel?: string,
): MenuEntry[] {
  const trigger =
    source.agentId === provenance.agentId &&
    source.status === 'ready' &&
    source.value.access !== 'shared'
      ? source.value.trigger
      : undefined;
  return [
    { type: 'label', id: 'creation-origin', text: originLabel ?? creationLabel(provenance) },
    ...(trigger === undefined
      ? []
      : [
          {
            id: 'published-trigger',
            label: `Published Trigger: ${trigger.title}`,
            href: `/t/${encodeURIComponent(trigger.public_id)}`,
          },
        ]),
  ];
}
