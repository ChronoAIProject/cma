import { useMemo, useSyncExternalStore } from 'react';
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
  let interval: ReturnType<typeof setInterval> | undefined;
  const publish = (next: Observation) => {
    state = next;
    listeners.forEach((listener) => listener());
  };
  const refresh = () => {
    if (active) return active;
    const abort = new AbortController();
    controller = abort;
    publish({ ...state, loading: true });
    let timer: ReturnType<typeof setTimeout>;
    active = Promise.resolve().then(async () => {
      try {
        if (!owner || !client.readSandboxCapacity) throw new Error(unavailable);
        const value = await Promise.race([
          client.readSandboxCapacity({ signal: abort.signal }),
          new Promise<never>((_, reject) => {
            timer = setTimeout(() => reject(new Error(unavailable)), READ_TIMEOUT_MS);
          }),
        ]);
        if (abort.signal.aborted) return undefined;
        publish({ value, checkedAt: Date.now(), loading: false });
        return value;
      } catch {
        if (!abort.signal.aborted) publish({ ...state, error: unavailable, loading: false });
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
  const visibleRefresh = () => {
    if (!document.hidden) void refresh();
  };
  return {
    snapshot: () => state,
    refresh,
    ensureFresh: () =>
      state.value &&
      !state.error &&
      Date.now() - (state.checkedAt ?? 0) < SANDBOX_CAPACITY_REFRESH_MS
        ? Promise.resolve(state.value)
        : refresh(),
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      if (listeners.size === 1) {
        visibleRefresh();
        interval = setInterval(visibleRefresh, SANDBOX_CAPACITY_REFRESH_MS);
        window.addEventListener('focus', visibleRefresh);
        document.addEventListener('visibilitychange', visibleRefresh);
      }
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0) {
          clearInterval(interval);
          window.removeEventListener('focus', visibleRefresh);
          document.removeEventListener('visibilitychange', visibleRefresh);
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
  const source = useMemo(() => sourceFor(client, owner), [client, owner]);
  const state = useSyncExternalStore(source.subscribe, source.snapshot);
  return { ...state, refresh: source.refresh, ensureFresh: source.ensureFresh };
}
