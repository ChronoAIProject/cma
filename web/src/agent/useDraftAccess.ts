import { useEffect, useRef } from 'react';
import type { AgentAccessValue } from '../resources/agentAccess';
import type { AuthorizedRead } from '../resources/useAuthorizedRead';
import { agentDraftKey, clearOwnerDrafts } from './draftStorage';
import { retainAgentDraftOwner } from './draftIdentity';

export function useDraftAccess(
  owner: string,
  agentId: string | undefined,
  access: AuthorizedRead<AgentAccessValue>,
  deleted: boolean,
) {
  const prior = useRef({ owner, personal: new Set<string>() });
  useEffect(() => {
    if (prior.current.owner !== owner) prior.current = { owner, personal: new Set() };
    if (!agentId) return;
    const detail = access.status === 'ready' ? access.value.detail : undefined;
    if (detail && detail.id !== agentId) return;
    const unavailable = deleted || detail?.status === 'deleted';
    if (detail && detail.access !== 'shared' && !unavailable) prior.current.personal.add(agentId);
    const lost =
      unavailable ||
      (access.status === 'error' && !access.transient) ||
      detail?.access === 'shared';
    if (!lost) return;
    if (!prior.current.personal.has(agentId)) {
      try {
        // A persisted personal draft retains the prior scope across reloads.
        if (sessionStorage.getItem(agentDraftKey(owner, agentId)) === null) return;
      } catch {
        return;
      }
    }
    prior.current.personal.clear();
    try {
      clearOwnerDrafts(owner);
    } catch {
      retainAgentDraftOwner();
    }
  }, [owner, agentId, access, deleted]);
}
