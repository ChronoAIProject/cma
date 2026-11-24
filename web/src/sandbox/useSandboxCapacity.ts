import { useCallback, useMemo, useSyncExternalStore } from 'react';
import { useSessionReadSignal } from '../auth/context';
import type { SandboxCapacity } from '@cma/client';
import type { CmaWebClient } from '../api/client';

export const SANDBOX_CAPACITY_REFRESH_MS = 10_000;
const READ_TIMEOUT_MS = 5_000;
const unavailable = 'Sandbox capacity could not be checked';
interface Observation {
  readonly value?: SandboxCapacity;
  readonly checkedAt?: number;
  readonly error?: string;
  readonly loading: boolean;
}

function capacitySource(client: CmaWebClient, owner?: string) {
  let state: Observation = { loading: false };
  const listeners = new Set<() => void>();
  let active: Promise<SandboxCapacity | undefined> | undefined;
  let controller: AbortController | undefined;
  let stopPolling: (() => void) | undefined;
  const publish = (next: Observation) => {
    state = next;
    listeners.forEach((listener) => listener());
  };
  const refresh = (sessionSignal?: AbortSignal) => {
    if (sessionSignal?.aborted) return Promise.resolve(undefined);
    if (active) return active;
    const abort = new AbortController();
    controller = abort;
    const signal = sessionSignal ? AbortSignal.any([abort.signal, sessionSignal]) : abort.signal;
    publish({ ...state, loading: true });
    let timer: ReturnType<typeof setTimeout>;
    active = Promise.resolve().then(async () => {
      try {
        signal.throwIfAborted();
        if (!owner || !client.readSandboxCapacity) throw new Error(unavailable);
        const value = await Promise.race([
          client.readSandboxCapacity({ signal }),
          new Promise<never>((_, reject) => {
            timer = setTimeout(() => reject(new Error(unavailable)), READ_TIMEOUT_MS);
          }),
        ]);
        if (signal.aborted) return undefined;
        publish({ value, checkedAt: Date.now(), loading: false });
        return value;
      } catch {
        if (!signal.aborted) publish({ ...state, error: unavailable, loading: false });
        return undefined;
      } finally {
        clearTimeout(timer!);
        abort.abort();
        if (controller === abort) {
          controller = undefined;
          active = undefined;
        }
      }
    });
    return active;
  };
  return {
    snapshot: () => state,
    refresh,
    ensureFresh: (sessionSignal?: AbortSignal) =>
      !sessionSignal?.aborted &&
      state.value &&
      !state.error &&
      Date.now() - (state.checkedAt ?? 0) < SANDBOX_CAPACITY_REFRESH_MS
        ? Promise.resolve(state.value)
        : refresh(sessionSignal),
    subscribe: (listener: () => void, sessionSignal?: AbortSignal) => {
      if (sessionSignal?.aborted) return () => {};
      listeners.add(listener);
      if (listeners.size === 1) {
        const visibleRefresh = () => {
          if (!document.hidden) void refresh(sessionSignal);
        };
        visibleRefresh();
        const interval = setInterval(visibleRefresh, SANDBOX_CAPACITY_REFRESH_MS);
        window.addEventListener('focus', visibleRefresh);
        document.addEventListener('visibilitychange', visibleRefresh);
        stopPolling = () => {
          clearInterval(interval);
          window.removeEventListener('focus', visibleRefresh);
          document.removeEventListener('visibilitychange', visibleRefresh);
        };
      }
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0) {
          stopPolling?.();
          stopPolling = undefined;
          controller?.abort();
          controller = undefined;
          active = undefined;
          state = { ...state, loading: false };
        }
      };
    },
  };
}

const sources = new WeakMap<
  CmaWebClient,
  Map<string | undefined, ReturnType<typeof capacitySource>>
>();
function sourceFor(client: CmaWebClient, owner?: string) {
  let owners = sources.get(client);
  if (!owners) {
    owners = new Map();
    sources.set(client, owners);
  }
  let source = owners.get(owner);
  if (!source) {
    source = capacitySource(client, owner);
    owners.set(owner, source);
  }
  return source;
}

export function refreshSandboxCapacity(client: CmaWebClient, owner: string) {
  return sourceFor(client, owner).refresh();
}

export function useSandboxCapacity(client: CmaWebClient, owner?: string) {
  const sessionSignal = useSessionReadSignal();
  const source = useMemo(() => sourceFor(client, owner), [client, owner]);
  const subscribe = useCallback(
    (listener: () => void) => source.subscribe(listener, sessionSignal),
    [source, sessionSignal],
  );
  const refresh = useCallback(() => source.refresh(sessionSignal), [source, sessionSignal]);
  const ensureFresh = useCallback(() => source.ensureFresh(sessionSignal), [source, sessionSignal]);
  const state = useSyncExternalStore(subscribe, source.snapshot);
  return { ...state, refresh, ensureFresh };
}
