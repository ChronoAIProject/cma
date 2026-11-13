import type { Capability } from '../auth/types';

export const operatorViews = [
  { id: 'sandboxes', label: 'Sandboxes & usage', capability: 'read_all_sandboxes' },
  { id: 'warm_pools', label: 'Warm pools', capability: 'configure_platform' },
  { id: 'settings', label: 'Platform settings', capability: 'configure_platform' },
  { id: 'activity', label: 'Activity', capability: 'read_activity_log' },
] as const;

export function canOpenOperations(capabilities: readonly Capability[]): boolean {
  return operatorViews.some((view) => capabilities.includes(view.capability));
}
