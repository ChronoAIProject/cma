import type { AnswerContext } from '@chronoai/cma-sdk';
import type { AgentController } from './controllerTypes';
import type { SteeringMode } from './SteeringMode';
import type { AgentV2Session } from './transport/client';
import type { useOptimisticMessages } from './optimisticMessages';
import type { useAgentConnectionReadiness } from './connectionReadiness';
import type { useConnectionGate } from '../connections/useConnectionGate';

export function messageSubmission({
  controller,
  optimistic,
  connection,
  connections,
  agentId,
  viewerId,
  readOnly,
  running,
  steeringMode,
  detailsOpen,
  v2,
}: {
  readonly controller: AgentController;
  readonly optimistic: ReturnType<typeof useOptimisticMessages>;
  readonly connection: ReturnType<typeof useAgentConnectionReadiness>;
  readonly connections: ReturnType<typeof useConnectionGate>;
  readonly agentId: string;
  readonly viewerId?: string;
  readonly readOnly: boolean;
  readonly running: boolean;
  readonly steeringMode: SteeringMode;
  readonly detailsOpen: boolean;
  readonly v2?: AgentV2Session;
}) {
  return (
    content: string,
    onAccepted: () => void,
    answerContext?: AnswerContext,
    onRefused?: (reason?: 'answer_changed' | 'suggestions_unavailable') => void,
  ) => {
    const verified = connection.verified;
    if (readOnly || verified === undefined) return;
    const message = optimistic.begin(
      content,
      running ? (steeringMode === 'queue' ? 'queue' : 'interrupt') : undefined,
      controller.queuedInputs?.items.map((item) => item.commandId),
    );
    return new Promise<void>((resolve) => {
      const settled = () => {
        message.settled();
        resolve();
      };
      const cancelled = (reason?: 'answer_changed' | 'suggestions_unavailable') => {
        message.cancelled();
        onRefused?.(reason);
        resolve();
      };
      const send = () => {
        const submit =
          running && steeringMode === 'interrupt'
            ? controller.interruptWithMessage
            : controller.send;
        if (submit === undefined) {
          cancelled();
          return;
        }
        const args: Parameters<AgentController['send']> = [
          content,
          (receipt) => {
            if (message.accepted(receipt)) onAccepted();
            resolve();
          },
          message.observed,
          cancelled,
          answerContext,
        ];
        const request =
          running && steeringMode === 'interrupt'
            ? controller.interruptWithMessage!(
                args[0],
                args[1],
                args[2],
                args[3],
                message.ordinaryInput,
              )
            : answerContext === undefined
              ? controller.send(args[0], args[1], args[2], args[3])
              : controller.send(...args);
        void request.then(settled, settled);
      };
      if (
        v2?.owner === (viewerId ?? verified.owner) &&
        controller.resourceAvailability.state === 'live'
      ) {
        send();
      } else {
        void connections.checkAndRun(
          { type: 'agent', id: agentId },
          viewerId ?? verified.owner,
          send,
          v2 !== undefined && !detailsOpen ? connection.takePrepared : undefined,
          cancelled,
        );
      }
    });
  };
}
