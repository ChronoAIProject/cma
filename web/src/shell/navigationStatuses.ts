import type { SandboxStatus, SandboxSummary } from '../api/types';
import { statusLabel } from '../vocabulary/status';
import { taskWords } from './taskStatus';

export type NavigationView = 'agent' | 'sandbox';
type SandboxNavigationGroup =
  'live' | 'changing' | 'paused' | 'needs_attention' | 'deleting' | 'hidden';

const sandboxStatusGroups = {
  active: 'live',
  paused: 'paused',
  failed: 'needs_attention',
  provisioning: 'changing',
  installing: 'changing',
  pausing: 'changing',
  reviving: 'changing',
  credential_rotating: 'changing',
  rotating: 'changing',
  retiring: 'deleting',
  retired: 'hidden',
} satisfies Record<SandboxStatus, SandboxNavigationGroup>;

export function sandboxNavigationGroup(
  sandbox: Pick<SandboxSummary, 'status' | 'lifecycle'>,
  deletionRequested = false,
): SandboxNavigationGroup {
  if (sandbox.status === 'retired') return 'hidden';
  if (sandbox.status === 'retiring' || deletionRequested) return 'deleting';
  if (sandbox.status === 'failed') return 'needs_attention';
  if (sandbox.lifecycle?.operation) return 'changing';
  return sandboxStatusGroups[sandbox.status];
}

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
        { id: 'changing', label: 'Changing' },
        { id: 'paused', label: 'Paused' },
        { id: 'needs_attention', label: 'Needs attention' },
      ];
}

export function sandboxFilterEmpty(status: string, onPage = false) {
  const label = navigationStatuses('sandbox').find((option) => option.value === status)?.label;
  return `No Sandboxes match ${label ?? 'this status'}${onPage ? ' on this page' : ''}.`;
}
