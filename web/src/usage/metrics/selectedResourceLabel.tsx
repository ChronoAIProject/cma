import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react';
import { metricsFailure, type MetricsFailure } from './failure';
import { retryUsageRead } from './retry';
import { useUsageVisible } from './useReport';

type LabelResult =
  | { readonly name: string; readonly error?: never }
  | { readonly error: MetricsFailure; readonly name?: never };

function labelStore() {
  const entries = new Map<string, LabelResult>();
  const listeners = new Set<() => void>();
  const notify = () => listeners.forEach((listener) => listener());
  return {
    get: (key: string) => entries.get(key),
    set(key: string, result: LabelResult) {
      entries.delete(key);
      entries.set(key, result);
      if (entries.size > 32) entries.delete(entries.keys().next().value!);
      notify();
    },
    remove(key: string) {
      entries.delete(key);
      notify();
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

const LabelsContext = createContext<ReturnType<typeof labelStore> | undefined>(undefined);

/** Scoped by the dashboard's owner/permission key; survives picker remounts, never persistence. */
export function SelectedResourceLabels({
  scope,
  children,
}: {
  readonly scope: object;
  readonly children: ReactNode;
}) {
  const store = useMemo(() => ({ scope, labels: labelStore() }), [scope]);
  return <LabelsContext value={store.labels}>{children}</LabelsContext>;
}

export function resourceLabelKey(kind: 'sandbox' | 'agent', id: string, sandbox = '') {
  return JSON.stringify([kind, sandbox, id]);
}

/** Cache only selected labels and settled failures; an aborted read can resume when visible. */
export function useSelectedResourceLabel({
  key,
  load,
  active,
  scope,
}: {
  readonly key: string;
  readonly load: (signal: AbortSignal) => Promise<string>;
  readonly active: boolean;
  readonly scope: object;
}) {
  const inherited = useContext(LabelsContext);
  const [local] = useState(labelStore);
  const store = inherited ?? local;
  const result = useSyncExternalStore(store.subscribe, () => store.get(key));
  const visible = useUsageVisible(active);
  useEffect(() => {
    if (!visible || result) return;
    const controller = new AbortController();
    void retryUsageRead(scope, controller.signal, load).then(
      (name) => {
        if (!controller.signal.aborted && !store.get(key)) store.set(key, { name });
      },
      (error: unknown) => {
        if (!controller.signal.aborted && !store.get(key))
          store.set(key, { error: metricsFailure(error) });
      },
    );
    return () => controller.abort();
  }, [key, load, result, scope, store, visible]);
  return {
    name: result?.name,
    error: result?.error,
    remember: (selectedKey: string, name: string) => store.set(selectedKey, { name }),
    retry: () => {
      if (!result?.error?.retryAt || Date.now() >= result.error.retryAt) store.remove(key);
    },
  };
}
