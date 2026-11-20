import { statusLabel } from '../vocabulary/status';
import { taskWords } from './taskStatus';

export type NavigationView = 'agent' | 'sandbox';
export const navigationStatuses = (view: NavigationView) => [
  { value: 'all', label: 'All statuses' },
  ...(view === 'agent'
    ? Object.entries(taskWords).map(([value, label]) => ({ value, label }))
    : (
        ['active', 'provisioning', 'installing', 'pausing', 'paused', 'reviving', 'failed'] as const
      ).map((value) => ({ value, label: statusLabel(value) }))),
];

export function sandboxNavigationGroups(status: string) {
  const selected = navigationStatuses('sandbox').find((option) => option.value === status);
  return selected && selected.value !== 'all'
    ? [{ id: status, label: selected.label }]
    : [
        { id: 'live', label: 'Live' },
        { id: 'paused', label: 'Paused' },
      ];
}

export function sandboxFilterEmpty(status: string, onPage = false) {
  const label = navigationStatuses('sandbox').find((option) => option.value === status)?.label;
  return `No Sandboxes match ${label ?? 'this status'}${onPage ? ' on this page' : ''}.`;
}
