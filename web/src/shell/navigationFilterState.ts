import { useState } from 'react';
import { navigationStatuses, type NavigationView } from './navigationStatuses';

interface Statuses {
  readonly agent: string;
  readonly sandbox: string;
}
interface SavedFilters {
  readonly statuses: Statuses;
  readonly unavailable: boolean;
}
const defaults: Statuses = { agent: 'all', sandbox: 'all' };

function storageKey(viewerId: string, scope: string) {
  return `cma:navigation-filters:v1:${JSON.stringify([location.origin, viewerId, scope])}`;
}

function valid(view: NavigationView, value: unknown): value is string {
  return navigationStatuses(view).some((option) => option.value === value);
}

function read(key: string): SavedFilters {
  let raw: string | null;
  try {
    raw = sessionStorage.getItem(key);
  } catch {
    return { statuses: defaults, unavailable: true };
  }
  try {
    const value: unknown = JSON.parse(raw ?? 'null');
    if (
      value &&
      typeof value === 'object' &&
      Object.keys(value).length === 2 &&
      'agent' in value &&
      'sandbox' in value &&
      valid('agent', value.agent) &&
      valid('sandbox', value.sandbox)
    )
      return { statuses: { agent: value.agent, sandbox: value.sandbox }, unavailable: false };
  } catch {
    /* Invalid stored preferences fall back to the default filters. */
  }
  return { statuses: defaults, unavailable: false };
}

function save(key: string, statuses: Statuses): SavedFilters {
  try {
    if (statuses.agent === 'all' && statuses.sandbox === 'all') sessionStorage.removeItem(key);
    else sessionStorage.setItem(key, JSON.stringify(statuses));
    return { statuses, unavailable: false };
  } catch {
    return { statuses, unavailable: true };
  }
}

export function useNavigationFilterState(viewerId: string, scope: string) {
  const key = storageKey(viewerId, scope);
  const [stored, setStored] = useState<Record<string, SavedFilters>>(() => ({ [key]: read(key) }));
  const current = stored[key] ?? read(key);
  if (!stored[key]) setStored({ ...stored, [key]: current });
  return {
    ...current,
    select(view: NavigationView, value: string, targetScope = scope) {
      if (!valid(view, value)) return;
      const targetKey = storageKey(viewerId, targetScope);
      const target = stored[targetKey] ?? read(targetKey);
      const next = save(targetKey, { ...target.statuses, [view]: value });
      setStored((previous) => ({ ...previous, [targetKey]: next }));
    },
    clear() {
      const cleared = { [key]: save(key, defaults) };
      if (scope !== '') {
        const personalKey = storageKey(viewerId, '');
        cleared[personalKey] = save(personalKey, defaults);
      }
      setStored((previous) => ({ ...previous, ...cleared }));
    },
  };
}
