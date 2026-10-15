import { Picker } from '../ui/Picker';
import { statusLabel } from '../vocabulary/status';
export type NavigationView = 'agent' | 'sandbox';
export const navigationStatuses = (view: NavigationView) => [
  { value: 'all', label: 'All statuses' },
  ...(view === 'agent'
    ? (['starting', 'ready', 'working', 'waiting_approval', 'paused', 'stopped', 'failed'] as const)
    : (['active', 'provisioning', 'installing', 'pausing', 'paused', 'reviving', 'failed'] as const)
  ).map((value) => ({ value, label: statusLabel(value) })),
];
export function NavigationFilters({
  view,
  status,
  onView,
  onStatus,
}: {
  readonly view: NavigationView;
  readonly status: string;
  readonly onView: (view: NavigationView) => void;
  readonly onStatus: (status: string) => void;
}) {
  return (
    <div className="navigation-filters">
      <div
        className="navigation-filters__views"
        role="group"
        aria-label="Navigation view"
        data-view={view}
      >
        <span className="navigation-filters__indicator" aria-hidden="true" />
        {(['agent', 'sandbox'] as const).map((value) => (
          <button
            type="button"
            key={value}
            aria-pressed={view === value}
            onClick={() => onView(value)}
          >
            {value === 'agent' ? 'Agent' : 'Sandbox'}
          </button>
        ))}
      </div>
      <Picker
        label={view === 'agent' ? 'Agent status filter' : 'Sandbox status filter'}
        value={status}
        options={navigationStatuses(view)}
        onChange={onStatus}
      />
    </div>
  );
}
