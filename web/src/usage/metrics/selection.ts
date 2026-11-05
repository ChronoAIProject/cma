import { useCallback, useMemo, useSyncExternalStore } from 'react';
import { defaultSelection, sorts, statuses, type Range, type Selection } from './model';

const ranges: readonly Range[] = ['1h', '24h', '7d', '30d', 'custom'];
const fields = [
  'range',
  'from',
  'to',
  'owner',
  'status',
  'runtime',
  'search',
  'sandbox',
  'agent',
  'sort',
  'order',
] as const;
export function readUsageSelection(search: string, admin: boolean): Selection {
  const values = new URLSearchParams(search);
  const selected = { ...defaultSelection };
  for (const field of ['from', 'to', 'runtime', 'search', 'sandbox', 'agent'] as const)
    selected[field] = (values.get(field) ?? '').slice(0, 255);
  const range = values.get('range');
  if (ranges.includes(range as Range)) selected.range = range as Range;
  if (admin) selected.owner = (values.get('owner') ?? '').slice(0, 255);
  const status = values.get('status');
  if (statuses.some((value) => value === status)) selected.status = status as Selection['status'];
  const sort = values.get('sort');
  if (sorts.includes(sort as Selection['sort']) && (admin || sort !== 'owner'))
    selected.sort = sort as Selection['sort'];
  if (values.get('order') === 'asc') selected.order = 'asc';
  return selected;
}
export function usageSearch(selection: Selection, admin: boolean, search = ''): string {
  const params = new URLSearchParams(search);
  for (const field of fields) {
    const value = field === 'owner' && !admin ? '' : selection[field];
    if (!value || value === defaultSelection[field]) params.delete(field);
    else params.set(field, value);
  }
  return params.toString();
}
function subscribe(listener: () => void) {
  window.addEventListener('popstate', listener);
  window.addEventListener('cma:usage-query', listener);
  return () => {
    window.removeEventListener('popstate', listener);
    window.removeEventListener('cma:usage-query', listener);
  };
}
export function useUsageSelection(admin: boolean) {
  const search = useSyncExternalStore(subscribe, () => window.location.search);
  const selection = useMemo(() => readUsageSelection(search, admin), [search, admin]);
  const update = useCallback(
    (next: Selection) => {
      const query = usageSearch(next, admin, window.location.search);
      window.history.replaceState(
        window.history.state,
        '',
        `${window.location.pathname}${query ? `?${query}` : ''}${window.location.hash}`,
      );
      window.dispatchEvent(new Event('cma:usage-query'));
    },
    [admin],
  );
  return [selection, update] as const;
}
