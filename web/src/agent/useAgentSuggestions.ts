import type { AgentController } from './controllerTypes';
import type { ConversationState } from './conversation';
import type { SuggestedReplySet } from './suggestions';
import { useContextualSuggestions } from './useContextualSuggestions';
import { useSuggestionEditing } from './useSuggestionEditing';
import { useSuggestionOutcomes } from './suggestionMetrics';
import { useCmaClient } from '../api/context';
import { useTaskExperience } from '../taskExperience';

export function useAgentSuggestions({
  conversation,
  viewerId,
  agentId,
  enabled,
  acceptedInput,
  provided,
  controller,
}: {
  conversation: ConversationState | undefined;
  viewerId: string;
  agentId: string;
  enabled: boolean;
  acceptedInput: boolean;
  provided?: SuggestedReplySet | null;
  controller: AgentController;
}) {
  const policy = useTaskExperience(useCmaClient(), viewerId);
  const selected = useContextualSuggestions(
    conversation,
    viewerId,
    agentId,
    enabled && !acceptedInput,
    provided,
  );
  useSuggestionOutcomes(
    conversation,
    viewerId,
    agentId,
    enabled ? controller.readSuggestionRequest?.() : null,
  );
  return useSuggestionEditing(selected, controller, policy.suggested_followup_admission);
}
