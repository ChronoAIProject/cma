import type { CmaWebClient } from '../api/client';
import type { useAgentActions } from './actions';

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
