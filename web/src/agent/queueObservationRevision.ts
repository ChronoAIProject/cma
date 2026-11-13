import type { ConversationState } from './model';

/** Usage/text/tool frames do not change queue admission; accepted inputs and turns can. */
export function queueObservationRevision(conversation: ConversationState) {
  let input = 0;
  for (let i = conversation.messages.length - 1; i >= 0; i--) {
    const message = conversation.messages[i];
    if (message?.role === 'user') {
      input = message.sequence ?? 0;
      break;
    }
  }
  return `${conversation.activeRunId ?? ''}:${input}:${conversation.agentStatus ?? ''}`;
}
