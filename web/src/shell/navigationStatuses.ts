import { sandboxStatusPresentation, type SandboxStatusObservation } from '../sandbox/presentation';
import { statusLabel } from '../vocabulary/status';
import { taskWords } from './taskStatus';

export type NavigationView = 'agent' | 'sandbox';
const sandboxFilters = [
  'active',
  'provisioning',
  'installing',
  'pausing',
  'paused',
  'reviving',
  'rotating',
  'credential_rotating',
  'failed',
] as const;

export const navigationStatuses = (view: NavigationView) => [
  { value: 'all', label: 'All statuses' },
  ...(view === 'agent'
    ? Object.entries(taskWords).map(([value, label]) => ({ value, label }))
    : sandboxFilters.map((value) => ({ value, label: statusLabel(value) }))),
];

function sandboxNavigationGroups(status: string) {
  const selected = navigationStatuses('sandbox').find((option) => option.value === status);
  return selected && selected.value !== 'all'
    ? [{ id: status, label: selected.label }]
    : [
        { id: 'live', label: 'Live' },
        { id: 'changing', label: 'Changing' },
        { id: 'paused', label: 'Paused' },
        { id: 'needs_attention', label: 'Needs attention' },
      ];
}

/** Group an already authorized inventory page without changing its pagination. */
export function sandboxNavigation<T>(
  items: readonly T[],
  sandboxOf: (item: T) => SandboxStatusObservation & { readonly id: string },
  filter: string,
  deletionRequested?: ReadonlySet<string>,
) {
  const groups = sandboxNavigationGroups(filter).map((group) => ({
    ...group,
    items: [] as T[],
  }));
  const byId = new Map(groups.map((group) => [group.id, group]));
  const visible: T[] = [];
  const deleting: T[] = [];
  for (const item of items) {
    const sandbox = sandboxOf(item);
    const { group, status } = sandboxStatusPresentation(
      sandbox,
      deletionRequested?.has(sandbox.id),
    );
    if (group === 'deleting') {
      deleting.push(item);
      continue;
    }
    if (
      group === 'hidden' ||
      (filter !== 'all' &&
        status !== filter &&
        !(filter === 'failed' && group === 'needs_attention'))
    )
      continue;
    byId.get(filter === 'all' ? group : filter)?.items.push(item);
    visible.push(item);
  }
  return { groups, visible, deleting };
}

export function sandboxFilterEmpty(status: string, onPage = false) {
  const label = navigationStatuses('sandbox').find((option) => option.value === status)?.label;
  return `No Sandboxes match ${label ?? 'this status'}${onPage ? ' on this page' : ''}.`;
}
