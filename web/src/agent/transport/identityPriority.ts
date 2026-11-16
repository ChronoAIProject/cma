import type { CurrentUser } from './identityGate';

type Role = 'foreground' | 'background';
interface Pending {
  readonly role: Role;
  readonly queuedAt: number;
  readonly request: AbortController;
  readonly signal?: AbortSignal;
  readonly resolve: (user: string | undefined) => void;
  readonly reject: (reason: unknown) => void;
  readonly abort: () => void;
  timer?: ReturnType<typeof setTimeout>;
  settled: boolean;
}

const maxQueued = 32;
const maxForegroundBurst = 2;
const backgroundAgeMs = 500;
const queueDeadlineMs = 5_000;

/** Optional per-transport pacing. No result sharing, identity cache or retry. */
export function createIdentityPriority(currentUser: CurrentUser, enabled = false) {
  if (!enabled) return { foreground: currentUser, background: currentUser };

  const queue: Pending[] = [];
  let active: Pending | undefined;
  let foregroundBurst = 0;

  const detach = (item: Pending) => {
    clearTimeout(item.timer);
    item.timer = undefined;
    item.signal?.removeEventListener('abort', item.abort);
  };
  const reject = (item: Pending, reason: unknown) => {
    if (item.settled) return;
    item.settled = true;
    detach(item);
    item.reject(reason);
  };
  const pump = () => {
    if (active !== undefined || queue.length === 0) return;
    const background = queue.findIndex((item) => item.role === 'background');
    const foreground = queue.findIndex((item) => item.role === 'foreground');
    const oldestBackground = queue[background];
    const backgroundDue =
      oldestBackground !== undefined &&
      (performance.now() - oldestBackground.queuedAt >= backgroundAgeMs ||
        foregroundBurst >= maxForegroundBurst);
    const index = backgroundDue ? background : foreground >= 0 ? foreground : 0;
    const item = queue.splice(index, 1)[0]!;
    if (item.signal?.aborted) {
      reject(item, item.signal.reason);
      pump();
      return;
    }
    if (performance.now() - item.queuedAt >= queueDeadlineMs) {
      reject(item, new Error('Identity verification queue deadline exceeded.'));
      pump();
      return;
    }
    clearTimeout(item.timer);
    item.timer = undefined;
    active = item;
    foregroundBurst =
      item.role === 'foreground' ? Math.min(maxForegroundBurst, foregroundBurst + 1) : 0;
    // Even an immediately resolved predecessor supplies no authority to this
    // call. Begin a new verification after this caller registered its check.
    void Promise.resolve()
      .then(() => {
        item.request.signal.throwIfAborted();
        return currentUser(item.request.signal);
      })
      .then(
        (user) => {
          if (item.settled) return;
          if (item.request.signal.aborted) reject(item, item.request.signal.reason);
          else {
            item.settled = true;
            detach(item);
            item.resolve(user);
          }
        },
        (error: unknown) => reject(item, error),
      )
      .finally(() => {
        // Cancellation settles its caller promptly, but cannot release this
        // slot until the probe settles. An abort-ignoring provider stays bound.
        active = undefined;
        pump();
      });
  };
  const check = (role: Role, signal?: AbortSignal): Promise<string | undefined> => {
    if (signal?.aborted) return Promise.reject(signal.reason);
    if (queue.length >= maxQueued)
      return Promise.reject(new Error('Identity verification queue is full.'));
    return new Promise((resolve, fail) => {
      const request = new AbortController();
      const item: Pending = {
        role,
        queuedAt: performance.now(),
        request,
        signal,
        resolve,
        reject: fail,
        settled: false,
        abort: () => {
          const index = queue.indexOf(item);
          if (index >= 0) queue.splice(index, 1);
          request.abort(signal?.reason);
          reject(item, signal?.reason);
        },
      };
      signal?.addEventListener('abort', item.abort, { once: true });
      item.timer = setTimeout(() => {
        const index = queue.indexOf(item);
        if (index < 0) return;
        queue.splice(index, 1);
        reject(item, new Error('Identity verification queue deadline exceeded.'));
      }, queueDeadlineMs);
      queue.push(item);
      pump();
    });
  };
  return {
    foreground: (signal?: AbortSignal) => check('foreground', signal),
    background: (signal?: AbortSignal) => check('background', signal),
  };
}
