import { useEffect } from 'react';
import { clearOwnerDrafts } from './draftStorage';
import { retainAgentDraftOwner } from './draftIdentity';

export function useDraftAccess(owner: string, revoked: boolean) {
  useEffect(() => {
    if (!revoked) return;
    try {
      clearOwnerDrafts(owner);
    } catch {
      retainAgentDraftOwner();
    }
  }, [owner, revoked]);
}
