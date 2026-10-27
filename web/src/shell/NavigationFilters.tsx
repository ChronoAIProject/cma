import { useId, useState } from 'react';
import type { ReactNode } from 'react';
import { Picker } from '../ui/Picker';
import { IconPersonalizationOutline16 } from '../ui/icons';
import { NavigationViews } from './NavigationViews';
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
 * and status only narrow it, so they sit behind one Filter key on the same
 * row. At their defaults nothing more is drawn: the dot and the summary line
 * appear only once something narrows the list, and the summary names only
 * what does, because restating the defaults is the noise this row removed.
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
  /** The chosen scope in words, for the summary line when it is not the default. */
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
  const summaryId = useId();
  const filtered = scopeActive || status !== 'all';
  const summary = [
    scopeActive ? scopeSummary : undefined,
    status === 'all'
      ? undefined
      : navigationStatuses(view).find((option) => option.value === status)?.label,
  ]
    .filter(Boolean)
    .join(' · ');
  const described = filtered && summary !== '';
  return (
    <div className="navigation-filters">
      <div className="navigation-filters__bar">
        <NavigationViews
          value={view}
          options={[
            { value: 'agent', label: 'Agents' },
            { value: 'sandbox', label: 'Sandboxes' },
          ]}
          onChange={onView}
        />
        <button
          type="button"
          className="navigation-filters__toggle"
          aria-label="Filter"
          title="Filter"
          aria-expanded={open}
          aria-controls={panelId}
          aria-describedby={described ? summaryId : undefined}
          onClick={() => setOpen((wasOpen) => !wasOpen)}
        >
          <IconPersonalizationOutline16 />
          {filtered ? <span className="navigation-filters__dot" aria-hidden="true" /> : null}
        </button>
      </div>
      {described ? (
        <p id={summaryId} className="navigation-filters__summary">
          {summary}
        </p>
      ) : null}
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
        {!filtered || onClear === undefined ? null : (
          <button type="button" className="navigation-filters__clear" onClick={onClear}>
            Clear filters
          </button>
        )}
      </div>
    </div>
  );
}
