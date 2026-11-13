export type CurrentUser = (signal?: AbortSignal) => Promise<string | undefined>;

interface Subscriber {
  readonly resolve: (user: string | undefined) => void;
  readonly reject: (error: unknown) => void;
  readonly detach: () => void;
}

interface Batch {
  readonly request: AbortController;
  readonly subscribers: Set<Subscriber>;
}

const counts = { requests: 0, joined: 0, handoffs: 0 };

/** Diagnostics contain counts only, never account identifiers or credentials. */
export const identityGateDiagnostics = () => ({ ...counts });

// Owner activation is separate from deployment. Reverting this constant keeps
// fresh request batching but restores every adjacent authorization round trip.
export const IDENTITY_HANDOFF_ENABLED = false;
const handoffLifetimeMs = 250;

/** A single adjacent continuation, never a reusable session cache. */
export function createIdentityHandoff(enabled = IDENTITY_HANDOFF_ENABLED) {
  const provedAt = performance.now();
  let available = true;
  return () => {
    const elapsed = performance.now() - provedAt;
    const valid = available && enabled && elapsed >= 0 && elapsed <= handoffLifetimeMs;
    available = false;
    if (valid) counts.handoffs += 1;
    return valid;
  };
}

/** One in-flight read and one queued batch; completed authority is never cached. */
export function createIdentityGate(currentUser: CurrentUser) {
  let running: Batch | undefined;
  let queued: Batch | undefined;

  const start = () => {
    if (running !== undefined || queued === undefined) return;
    const batch = queued;
    queued = undefined;
    if (batch.subscribers.size === 0) return;
    running = batch;
    counts.requests += 1;
    const settle = (complete: (subscriber: Subscriber) => void) => {
      for (const subscriber of batch.subscribers) {
        subscriber.detach();
        complete(subscriber);
      }
      batch.subscribers.clear();
      running = undefined;
      // Start only after queued callers have registered. No caller can join a
      // request that began before its own check, even when results resolve fast.
      queueMicrotask(start);
    };
    try {
      void currentUser(batch.request.signal).then(
        (user) => settle((subscriber) => subscriber.resolve(user)),
        (error: unknown) => settle((subscriber) => subscriber.reject(error)),
      );
    } catch (error) {
      settle((subscriber) => subscriber.reject(error));
    }
  };

  return {
    check(signal?: AbortSignal): Promise<string | undefined> {
      if (signal?.aborted) return Promise.reject(signal.reason);
      if (queued === undefined) queued = { request: new AbortController(), subscribers: new Set() };
      else counts.joined += 1;
      const batch = queued;
      const result = new Promise<string | undefined>((resolve, reject) => {
        const subscriber: Subscriber = {
          resolve,
          reject,
          detach: () => signal?.removeEventListener('abort', abort),
        };
        const abort = () => {
          subscriber.detach();
          batch.subscribers.delete(subscriber);
          reject(signal?.reason);
          if (batch.subscribers.size === 0) {
            batch.request.abort();
            if (queued === batch) queued = undefined;
          }
        };
        batch.subscribers.add(subscriber);
        signal?.addEventListener('abort', abort, { once: true });
      });
      queueMicrotask(start);
      return result;
    },
  };
}
