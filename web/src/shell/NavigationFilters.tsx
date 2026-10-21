import { useId, useState } from 'react';
import type { ReactNode } from 'react';
import { Picker } from '../ui/Picker';
import { IconChevronDownOutline14, IconChevronUpOutline14 } from '../ui/icons';
import { statusLabel } from '../vocabulary/status';
export type NavigationView = 'agent' | 'sandbox';
export const navigationStatuses = (view: NavigationView) => [
  { value: 'all', label: 'All statuses' },
  ...(view === 'agent'
    ? (['starting', 'ready', 'working', 'waiting_approval', 'paused', 'stopped', 'failed'] as const)
    : (['active', 'provisioning', 'installing', 'pausing', 'paused', 'reviving', 'failed'] as const)
  ).map((value) => ({ value, label: statusLabel(value) })),
];

/**
 * The navigation view and its filters — `docs/Web_Style.md` §10.
 *
 * The view switch stays visible because it changes what the list *is*. Scope
 * and status only narrow it, so they sit behind one Filter button whose count
 * and summary line say what is applied, and the list gets their row back.
 */
export function NavigationFilters({
  scope,
  scopeSummary,
  scopeActive = false,
  view,
  status,
  onView,
  onStatus,
  onClear,
}: {
  readonly scope?: ReactNode;
  /** The chosen scope in words, for the summary line. */
  readonly scopeSummary?: string;
  /** Whether the scope differs from the default, Personal. */
  readonly scopeActive?: boolean;
  readonly view: NavigationView;
  readonly status: string;
  readonly onView: (view: NavigationView) => void;
  readonly onStatus: (status: string) => void;
  readonly onClear?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const statusSummary =
    navigationStatuses(view).find((option) => option.value === status)?.label ?? 'All statuses';
  const applied = (status === 'all' ? 0 : 1) + (scopeActive ? 1 : 0);
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
            {value === 'agent' ? 'Agents' : 'Sandboxes'}
          </button>
        ))}
      </div>
      <div className="navigation-filters__bar">
        <span className="navigation-filters__summary">
          {[scopeSummary, statusSummary].filter(Boolean).join(' · ')}
        </span>
        <button
          type="button"
          className="navigation-filters__toggle"
          aria-expanded={open}
          aria-controls={panelId}
          aria-label={applied === 0 ? 'Filter' : `Filter, ${String(applied)} applied`}
          onClick={() => setOpen((wasOpen) => !wasOpen)}
        >
          <span>Filter</span>
          {applied === 0 ? null : (
            <span className="navigation-filters__count" aria-hidden="true">
              {applied}
            </span>
          )}
          {open ? <IconChevronUpOutline14 /> : <IconChevronDownOutline14 />}
        </button>
      </div>
      <div
        id={panelId}
        className="navigation-filters__panel"
        role="group"
        aria-label="Navigation filters"
        hidden={!open}
      >
        {scope === undefined ? null : (
          <div className="navigation-filters__field">
            <span className="navigation-filters__label" aria-hidden="true">
              Scope
            </span>
            {scope}
          </div>
        )}
        <div className="navigation-filters__field">
          <span className="navigation-filters__label" aria-hidden="true">
            Status
          </span>
          <Picker
            label={view === 'agent' ? 'Agent status filter' : 'Sandbox status filter'}
            placement="navigation-filters__picker"
            value={status}
            options={navigationStatuses(view)}
            onChange={onStatus}
          />
        </div>
        {applied === 0 || onClear === undefined ? null : (
          <button type="button" className="navigation-filters__clear" onClick={onClear}>
            Clear filters
          </button>
        )}
      </div>
    </div>
  );
}
