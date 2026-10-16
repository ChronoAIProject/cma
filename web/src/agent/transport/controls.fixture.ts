import type { V2ResponseControlRequest, V2ResponseInterruptionRequest } from '@chronoai/cma-sdk';
const at = '2026-09-16T00:00:00Z';

export function commandStatus(responseId: string | null, turn = 1, completed = false) {
  return {
    object: 'cma.agent.command',
    receipt: {
      command_id: responseId === null ? 'cmd_control' : 'cmd_1',
      agent_id: 'agt_1',
      response_id: responseId,
      admitted_at: at,
      agent_order: 1,
      event_cursor: 1,
    },
    status: completed ? 'completed' : 'in_progress',
    queue_deadline: at,
    turn_sequence: turn,
    observed_cursor: 1,
    delivery: 'execution_observed',
    failure: null,
  };
}

export function observation(responseId = 'resp_1', turn = 1, revision = 10) {
  return {
    object: 'cma.response.controls',
    response: commandStatus(responseId, turn),
    target: { response_id: responseId, turn_sequence: turn },
    agent_revision: revision,
    target_active: true,
    runtime_support: { cancel: true, steer: true, resolve_approval: true },
  };
}

export function controlReceipt(responseId: string, request: V2ResponseControlRequest) {
  return {
    object: 'cma.response.control',
    control: commandStatus(null, request.turn_sequence, true),
    target: { response_id: responseId, turn_sequence: request.turn_sequence },
    expected_agent_revision: request.expected_agent_revision,
  };
}

export function interruptionReceipt(responseId: string, request: V2ResponseInterruptionRequest) {
  const message = commandStatus('resp_interrupted_next');
  return {
    object: 'cma.response.interruption',
    control: commandStatus(null, request.turn_sequence, true),
    response: {
      ...message,
      receipt: { ...message.receipt, command_id: 'cmd_interrupted_next', agent_order: 2 },
      status: 'queued',
      turn_sequence: null,
      observed_cursor: null,
      delivery: null,
    },
    target: { response_id: responseId, turn_sequence: request.turn_sequence },
    expected_agent_revision: request.expected_agent_revision,
  };
}
