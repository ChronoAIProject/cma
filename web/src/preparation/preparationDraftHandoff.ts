import type { AgentDraft } from '../agent/draft';
import {
  readPreparationDraft,
  removePreparationDraft,
  type PreparationDraftBinding,
  type PreparationDraftRecord,
} from './preparationDraftStorage';

export type PreparationDraftTransfer = 'transferred' | 'retained';

/** Copy and verify before removing this exact source; equal text is not a transfer receipt. */
export function transferPreparationDraft(
  source: PreparationDraftRecord,
  owner: string,
  binding: PreparationDraftBinding,
  destination: AgentDraft,
  authorized: () => boolean,
): PreparationDraftTransfer {
  const sourceCurrent = () =>
    authorized() &&
    source.scope.owner === owner &&
    JSON.stringify(source.binding) === JSON.stringify(binding) &&
    JSON.stringify(readPreparationDraft(source.scope)) === JSON.stringify(source);
  try {
    if (!source.text || !sourceCurrent()) return 'retained';
    const receipt = destination.importText?.(
      { owner, agentId: binding.agentId, text: source.text },
      sourceCurrent,
    );
    if (!receipt || !sourceCurrent() || !receipt.verify()) return 'retained';
    removePreparationDraft(source.scope, source);
    return 'transferred';
  } catch {
    return 'retained';
  }
}
