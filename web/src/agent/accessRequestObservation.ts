import { rateLimitDelay } from './transport/readRetry';
import type { AgentAccessRequest } from './accessRequests';
import { observeRead } from './transport/readObserver';

const activeDelay = 2_000;
const runningLimit = 15_000;
// Runtime consent admission checks live lineage, not an active turn. Owner
// service-access updates can also create requests while a conversation is idle.
const idleDelay = 30_000;

export function observeAccessRequests(options: {
  readonly running: () => boolean;
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
        const running = options.running();
        const summary = JSON.stringify([
          running,
          [...requests].sort((a, b) => a.id.localeCompare(b.id)),
        ]);
        unchanged = previous === summary ? unchanged + 1 : 0;
        previous = summary;
        failures = 0;
        options.publish(requests);
        const pending = requests.some(({ status }) =>
          ['pending', 'connecting', 'applying'].includes(status),
        );
        const next = pending
          ? activeDelay
          : running
            ? Math.min(activeDelay * 2 ** Math.min(unchanged, 3), runningLimit)
            : idleDelay;
        // Conversation events are hints, including ordinary text and tool
        // updates. Keep them within the current polling cadence.
        return { next, cooldown: next };
      } catch (error) {
        if (signal.aborted) return undefined;
        options.failed(error);
        const next = rateLimitDelay(
          error,
          Math.min(activeDelay * 2 ** Math.min(failures++, 3), 10000),
          failures,
        );
        return { next, cooldown: next, failed: true };
      }
    },
    { immediateForeground: true },
  );
}
