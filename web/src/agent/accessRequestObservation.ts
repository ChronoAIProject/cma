import type { AgentAccessRequest } from './accessRequests';
import { observeRead } from './transport/readObserver';

const activeDelay = 2_000;
const runningLimit = 15_000;
const retryLimit = 10_000;
// Runtime consent admission checks live lineage, not an active turn. Owner
// service-access updates can also create requests while a conversation is idle.
const idleDelay = 30_000;

export function observeAccessRequests(options: {
  readonly running: boolean;
  readonly read: (signal: AbortSignal) => Promise<readonly AgentAccessRequest[]>;
  readonly publish: (requests: readonly AgentAccessRequest[]) => void;
  readonly failed: (error: unknown) => void;
}) {
  let previous: string | undefined;
  let unchanged = 0;
  let failures = 0;
  return observeRead(
    async (signal) => {
      try {
        const requests = await options.read(signal);
        signal.throwIfAborted();
        const summary = JSON.stringify([...requests].sort((a, b) => a.id.localeCompare(b.id)));
        unchanged = previous === summary ? unchanged + 1 : 0;
        previous = summary;
        failures = 0;
        options.publish(requests);
        const pending = requests.some(({ status }) =>
          ['pending', 'connecting', 'applying'].includes(status),
        );
        const next = pending
          ? activeDelay
          : options.running
            ? Math.min(activeDelay * 2 ** Math.min(unchanged, 3), runningLimit)
            : idleDelay;
        return { next, cooldown: activeDelay };
      } catch (error) {
        if (signal.aborted) return undefined;
        options.failed(error);
        const next = Math.min(activeDelay * 2 ** Math.min(failures++, 3), retryLimit);
        return { next, cooldown: next, failed: true };
      }
    },
    { immediateForeground: true },
  );
}
