import { ApiError, RequestAborted, TransportError } from '@cma/client';
import { AgentFileError } from './agentFiles';

export interface ReadRetryMetadata {
  readonly shape: 'problem' | 'legacy-empty' | 'invalid';
  readonly retryable?: boolean;
  readonly retryAfterMs?: number;
  readonly requestId?: string;
  /** Monotonic deadline captured when response headers arrived. */
  readonly retryAt?: number;
}

interface Floors {
  readonly entries: Map<string, number>;
  overflow: number;
}
const clients = new WeakMap<object, Floors>();
const received = new WeakMap<object, number>();
const observing = new WeakMap<AbortSignal, (at: number) => void>();
const MAX_DELAY = 2_147_483_647;

function scope(client: object): Floors {
  let value = clients.get(client);
  if (!value) {
    value = { entries: new Map(), overflow: 0 };
    clients.set(client, value);
  }
  const now = performance.now();
  for (const [key, at] of value.entries) if (at <= now) value.entries.delete(key);
  if (value.overflow <= now) value.overflow = 0;
  return value;
}

/** Publish header timing before decoding can yield to a replacement read. */
export function observeRetryAfter(
  delay: number | undefined,
  signal?: AbortSignal,
): number | undefined {
  if (delay === undefined || !Number.isSafeInteger(delay) || delay <= 0 || delay > MAX_DELAY)
    return undefined;
  const at = performance.now() + delay;
  if (signal) observing.get(signal)?.(at);
  return at;
}

function remember(client: object, resourceKey: string, at: number) {
  if (!Number.isFinite(at) || at <= performance.now()) return;
  const floors = scope(client);
  if (floors.entries.has(resourceKey) || floors.entries.size < 64)
    floors.entries.set(resourceKey, Math.max(floors.entries.get(resourceKey) ?? 0, at));
  else floors.overflow = Math.max(floors.overflow, at);
}

function metadata(error: ApiError | AgentFileError): ReadRetryMetadata | undefined {
  return (error as typeof error & { readonly problem?: ReadRetryMetadata }).problem;
}

function deadline(error: unknown): number {
  if (typeof error !== 'object' || error === null) return 0;
  if (received.has(error)) return received.get(error)!;
  if (!(error instanceof ApiError || error instanceof AgentFileError)) return 0;
  const problem = metadata(error);
  const delay = error.retryAfterMs;
  const at = problem
    ? (problem.retryAt ?? 0)
    : typeof delay === 'number' && Number.isSafeInteger(delay) && delay > 0 && delay <= MAX_DELAY
      ? performance.now() + delay
      : 0;
  received.set(error, at);
  return at;
}

function wait(milliseconds: number, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const finish = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
    };
    const abort = () => {
      finish();
      reject(signal.reason);
    };
    const timer = setTimeout(
      () => {
        finish();
        resolve();
      },
      Math.min(MAX_DELAY, Math.ceil(milliseconds)),
    );
    signal.addEventListener('abort', abort, { once: true });
  });
}

/** Timing only, shared across live callers. Each run dispatches at most one read. */
export function readGate(client: object, resourceKey: string) {
  return {
    async run<T>(signal: AbortSignal, read: (signal: AbortSignal) => Promise<T>): Promise<T> {
      for (;;) {
        signal.throwIfAborted();
        const floors = scope(client);
        const remaining =
          Math.max(floors.entries.get(resourceKey) ?? 0, floors.overflow) - performance.now();
        if (remaining <= 0) break;
        await wait(remaining, signal);
      }
      signal.throwIfAborted();
      const request = new AbortController();
      const abort = () => request.abort(signal.reason);
      signal.addEventListener('abort', abort, { once: true });
      const record = (at: number) => remember(client, resourceKey, at);
      observing.set(request.signal, record);
      try {
        const value = await read(request.signal);
        signal.throwIfAborted();
        return value;
      } catch (error) {
        record(deadline(error));
        throw error;
      } finally {
        signal.removeEventListener('abort', abort);
        observing.delete(request.signal);
      }
    },
  };
}

export function retryLifecycleRead(error: unknown, signal: AbortSignal): boolean {
  if (signal.aborted || error instanceof RequestAborted) return false;
  if (error instanceof ApiError) {
    if (metadata(error)?.shape === 'invalid') return false;
    const retryable = error.extensions.retryable;
    if (retryable !== undefined && typeof retryable !== 'boolean') return false;
    if (retryable === false) return false;
    return error.isRetryable || error.status === 408;
  }
  return error instanceof TransportError;
}
