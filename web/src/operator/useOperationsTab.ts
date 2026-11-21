import { useCallback, useEffect, useSyncExternalStore } from 'react';

function subscribe(listener: () => void) {
  window.addEventListener('popstate', listener);
  window.addEventListener('cma:operations-tab', listener);
  return () => {
    window.removeEventListener('popstate', listener);
    window.removeEventListener('cma:operations-tab', listener);
  };
}
export function useOperationsTab() {
  const search = useSyncExternalStore(subscribe, () => window.location.search);
  const raw = new URLSearchParams(search).get('tab');
  const selected = raw === 'usage' ? 'sandboxes' : (raw ?? 'sandboxes');
  const setSelected = useCallback((value: string) => {
    const query = new URLSearchParams(window.location.search);
    if (value === 'sandboxes') query.delete('tab');
    else query.set('tab', value);
    window.history.replaceState(
      window.history.state,
      '',
      window.location.pathname + (query.size ? `?${query}` : '') + window.location.hash,
    );
    window.dispatchEvent(new Event('cma:operations-tab'));
  }, []);
  useEffect(() => {
    if (raw === 'usage') setSelected('sandboxes');
  }, [raw, setSelected]);
  return [selected, setSelected] as const;
}
