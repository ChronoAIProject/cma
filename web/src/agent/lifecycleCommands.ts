import type { CmaWebClient } from '../api/client';
import type { useAgentActions } from './actions';
import type { AgentResourceState } from './controllerTypes';

export function sameStopResponse(
  displayed: AgentResourceState,
  current: AgentResourceState,
): boolean {
  const displayedV2 = displayed.status === 'ready' ? displayed.v2 : undefined;
  const currentV2 = current.status === 'ready' ? current.v2 : undefined;
  if (displayedV2 === undefined && currentV2 === undefined) return true;
  if (
    displayed.status !== 'ready' ||
    current.status !== 'ready' ||
    displayedV2 !== currentV2 ||
    displayed.detail.id !== current.detail.id ||
    displayed.conversation.activeRunId !== current.conversation.activeRunId
  )
    return false;
  const shown = displayed.conversation.responseBinding;
  const live = current.conversation.responseBinding;
  return (
    shown?.responseId === live?.responseId &&
    shown?.turnSequence === live?.turnSequence &&
    (shown === undefined || current.conversation.activeRunId === String(shown.turnSequence))
  );
}

export function lifecycleCommands(
  client: CmaWebClient,
  command: ReturnType<typeof useAgentActions>['command'],
) {
  const interrupt = (strict: boolean) =>
    command(
      'interrupt',
      (id, key, signal) => client.interruptAgent(id, { idempotencyKey: key, signal }),
      strict,
    );
  const restartAgent = client.restartAgent?.bind(client);
  return {
    interruptForControl: () => interrupt(true),
    interrupt: () => interrupt(false),
    stop: () =>
      command('stop', (id, key, signal) => client.stopAgent(id, { idempotencyKey: key, signal })),
    restart:
      restartAgent &&
      (() =>
        command('restart', (id, key, signal) => restartAgent(id, { idempotencyKey: key, signal }))),
    delete: () =>
      command('delete', (id, key, signal) =>
        client.deleteAgent(id, { idempotencyKey: key, signal }),
      ),
  };
}
