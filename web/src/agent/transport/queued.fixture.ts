import type { V2QueuedCancellationRequest } from '@cma/client';
import { commandStatus } from './controls.fixture';

export function queuedObservation(responseId = 'resp_1', revision = 7, cancellable = true) {
  return {
    object: 'cma.response.queued_cancellation.observation',
    response: {
      ...commandStatus(responseId),
      status: cancellable ? 'queued' : 'completed',
      turn_sequence: null,
      observed_cursor: null,
      delivery: null,
    },
    command_revision: revision,
    cancellable,
  };
}

export function queuedReceipt(responseId: string, body: V2QueuedCancellationRequest) {
  return {
    object: 'cma.response.queued_cancellation',
    cancellation: {
      ...commandStatus(null),
      status: 'completed',
      turn_sequence: null,
      observed_cursor: null,
      delivery: null,
    },
    response: { ...queuedObservation(responseId).response, status: 'cancelled' },
    target: { response_id: responseId, ...body },
  };
}
