interface QueuedRead {
  readonly background: boolean;
  readonly start: () => void;
}
interface Queue {
  active: boolean;
  pending: QueuedRead[];
}
const queues = new WeakMap<object, Queue>();

/** Serialize report observations; visible selections take priority over range warming. */
export function reportRequest<T>(
  client: object,
  signal: AbortSignal,
  load: () => Promise<T>,
  background = false,
): Promise<T> {
  signal.throwIfAborted();
  let queue = queues.get(client);
  if (!queue) {
    queue = { active: false, pending: [] };
    queues.set(client, queue);
  }
  const scope = queue;
  return new Promise((resolve, reject) => {
    const cancelled = () => {
      scope.pending = scope.pending.filter((item) => item !== queued);
      reject(signal.reason);
    };
    const queued: QueuedRead = {
      background,
      start: () => {
        signal.removeEventListener('abort', cancelled);
        scope.active = true;
        let finished = false;
        const finish = () => {
          if (finished) return;
          finished = true;
          signal.removeEventListener('abort', aborted);
          scope.active = false;
          drain(scope);
        };
        const aborted = () => {
          reject(signal.reason);
          finish();
        };
        signal.addEventListener('abort', aborted, { once: true });
        // The SDK cancels fetch on abort. Settling this owner immediately also
        // fences late responses from test/custom transports that ignore it.
        void Promise.resolve()
          .then(() => {
            signal.throwIfAborted();
            return load();
          })
          .then(resolve, reject)
          .finally(finish);
      },
    };
    signal.addEventListener('abort', cancelled, { once: true });
    scope.pending.push(queued);
    drain(scope);
  });
}
function drain(queue: Queue) {
  if (queue.active) return;
  const index = queue.pending.findIndex((item) => !item.background);
  const [next] = queue.pending.splice(index < 0 ? 0 : index, 1);
  next?.start();
}
