import { useId } from 'react';
import type { ReactNode } from 'react';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';
import { IconPersonalizationOutline16 } from '../ui/icons';
import { navigationStatuses, type NavigationView } from './navigationStatuses';
export { navigationStatuses, type NavigationView } from './navigationStatuses';

export function NavigationFilters({
  scope,
  scopeSummary,
  scopeActive = false,
  view,
  onView,
  open,
  onOpenChange,
  status,
  onStatus,
  onClear,
}: {
  readonly scope?: ReactNode;
  /** The chosen scope in words, for the summary line when it is not the default. */
  readonly scopeSummary?: string;
  /** Whether the scope differs from the default, Personal. */
  readonly scopeActive?: boolean;
  readonly view: NavigationView;
  readonly onView: (view: NavigationView) => void;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly status: string;
  readonly onStatus: (status: string) => void;
  readonly onClear?: () => void;
}) {
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
        <div className="navigation-filters__views" role="group" aria-label="Navigation view">
          {(['agent', 'sandbox'] as const).map((option) => (
            <Button
              key={option}
              size="sm"
              variant={view === option ? 'toolbar' : 'ghost'}
              aria-pressed={view === option}
              data-navigation-mode={option}
              onClick={() => onView(option)}
            >
              {option === 'agent' ? 'Tasks' : 'Environments'}
            </Button>
          ))}
        </div>
        <button
          type="button"
          className="navigation-filters__toggle"
          aria-label="Filter"
          title="Filter"
          aria-expanded={open}
          aria-controls={panelId}
          aria-describedby={described ? summaryId : undefined}
          onClick={() => onOpenChange(!open)}
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
