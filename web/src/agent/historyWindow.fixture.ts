import type {
  V2AgentEventItem,
  V2AgentHistoryWindow,
  V2AgentStatePage,
  V2AgentCursor,
} from '@chronoai/cma-sdk';
import {
  binding,
  cursor as rawCursor,
  incarnation,
  item as fixtureItem,
} from './transport/controller.fixture';

const cursor = (sequence: number) => rawCursor(sequence) as V2AgentCursor;
export const historyItem = (sequence: number, kind: unknown, response: unknown = null) =>
  fixtureItem(sequence, kind, response) as V2AgentEventItem;
const turn = historyItem(1, { type: 'turn_started', turn_sequence: 1 }, binding);
export function historyPage(after = 900, through = 902): V2AgentHistoryWindow {
  return {
    object: 'cma.agent.history_window',
    schema_version: 1,
    agent_id: 'agt_1',
    incarnation,
    start_cursor: cursor(after),
    through_cursor: cursor(through),
    context: turn,
    gap: null,
    previous_cursor: cursor(after),
    items: Array.from({ length: through - after }, (_, n) =>
      historyItem(after + n + 1, {
        type: 'message',
        role: 'assistant',
        content: `Reply ${after + n + 1}`,
      }),
    ),
  };
}
export function statePage(through = 902): V2AgentStatePage {
  return {
    object: 'cma.agent.state_page',
    schema_version: 1,
    agent_id: 'agt_1',
    incarnation,
    through_cursor: cursor(through),
    context: [
      turn,
      historyItem(5, { type: 'available_commands_set', commands: [] }),
      historyItem(7, { type: 'plan_set', entries: [{ title: 'Current work', status: 'pending' }] }),
    ],
    approvals: [
      historyItem(6, {
        type: 'approval_requested',
        approval_id: 'approval-1',
        action: 'fixture',
        detail: 'Original request',
      }),
    ],
    next_approval_cursor: null,
  };
}
