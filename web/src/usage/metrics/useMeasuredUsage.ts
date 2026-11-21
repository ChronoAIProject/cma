import { useEffect, useMemo, useState } from 'react';
import { useUsageVisible } from './useReport';

export type ReadMeasuredUsage = (options: { readonly signal: AbortSignal }) => Promise<boolean>;

/** Successful platform saves invalidate the display setting without changing a workload. */
export function usageSettingsChanged(): void {
  window.dispatchEvent(new Event('cma:usage-settings'));
}

/** Only an authoritative effective true enables measurement reads; failures hide them. */
export function useMeasuredUsage(read: ReadMeasuredUsage | undefined, active = true): boolean {
  const visible = useUsageVisible(active);
  const context = useMemo(() => ({ read, visible }), [read, visible]);
  const [state, setState] = useState<{ context: typeof context; enabled: boolean }>();
  useEffect(() => {
    if (!visible || !read) return;
    let controller: AbortController | undefined;
    const refresh = () => {
      if (controller) return;
      const request = new AbortController();
      controller = request;
      void read({ signal: request.signal })
        .then(
          (enabled) => {
            if (!request.signal.aborted) setState({ context, enabled: enabled === true });
          },
          () => {
            if (!request.signal.aborted) setState({ context, enabled: false });
          },
        )
        .finally(() => {
          if (controller === request) controller = undefined;
        });
    };
    const changed = () => {
      controller?.abort();
      controller = undefined;
      setState({ context, enabled: false });
      refresh();
    };
    refresh();
    window.addEventListener('cma:usage-settings', changed);
    const timer = setInterval(() => refresh(), 60000);
    return () => {
      controller?.abort();
      window.removeEventListener('cma:usage-settings', changed);
      clearInterval(timer);
    };
  }, [read, visible, context]);
  return visible && state?.context === context && state?.enabled === true;
}
