import type { AgentCreationCategory, AgentProvenance } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';
import { useAgentFact, type AgentFactState } from './useAgentFact';

export type ProvenanceState = AgentFactState<AgentProvenance>;

export const creationLabels: Readonly<Record<AgentCreationCategory, string>> = {
  manual: 'Created manually',
  trigger: 'Created by Trigger',
  channel_bridge: 'Created by channel bridge',
  legacy_unknown: 'Creation origin unknown',
};

export function creationLabel(provenance: ProvenanceState): string {
  if (provenance.status === 'ready') return creationLabels[provenance.value.creation_origin];
  return provenance.status === 'loading'
    ? 'Loading creation origin'
    : 'Creation origin unavailable';
}

export function useAgentProvenance(agentId: string, enabled: boolean): ProvenanceState {
  return useAgentFact(agentId, enabled, useCmaClient().readAgentProvenance);
}
