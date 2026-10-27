import type { AgentCreationCategory, AgentProvenance } from '@chronoai/cma-sdk';
import { useEffect } from 'react';
import { useCmaClient } from '../api/context';
import { useOwnerViews } from '../resources/retainedOwnerViews';
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

/** Retain only the immutable category; privileged provenance/source facts still revalidate. */
export function useOwnerCreationLabel(
  provenance: ProvenanceState,
  owner: string | undefined,
  visible: boolean,
): string {
  const views = useOwnerViews();
  const { agentId } = provenance;
  const differentOwner = owner !== undefined && owner !== views?.viewerId;
  useEffect(() => {
    if (!visible || differentOwner || provenance.status === 'unavailable')
      views?.origins.delete(agentId);
    else if (provenance.status === 'ready' && owner === views?.viewerId)
      views?.origins.put(agentId, provenance.value.creation_origin);
  }, [views, provenance, agentId, owner, visible, differentOwner]);
  const retained =
    visible && !differentOwner && provenance.status === 'loading'
      ? views?.origins.get(agentId)
      : undefined;
  return retained === undefined ? creationLabel(provenance) : creationLabels[retained];
}
