import { useEffect, useId, useRef, type RefObject } from 'react';
import { useOperationsTab } from './useOperationsTab';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import type { Capability } from '../auth/types';
import type { OperatorClient } from './client';
import { Sandboxes } from './Sandboxes';
import { BindingHolds } from './BindingHolds';
import { PlatformSettings } from './PlatformSettings';
import { WarmPools } from './WarmPools';
import { Activity } from './Activity';
import { operatorViews } from './capabilities';
import { defaultSelection, type MetricsClient } from '../usage/metrics/model';
import { prefetchUsageReport } from '../usage/metrics/useReport';
import { anonymousReportOwner, type UsageReportOwner } from '../usage/metrics/reportOwner';
import './Operations.css';

export { canOpenOperations } from './capabilities';

export function Operations({
  client,
  capabilities,
  principal = anonymousReportOwner,
  bindingDiagnostics = false,
}: {
  readonly client: OperatorClient;
  readonly capabilities: readonly Capability[];
  readonly principal?: UsageReportOwner;
  readonly bindingDiagnostics?: boolean;
}) {
  const available = operatorViews.filter((view) => capabilities.includes(view.capability));
  const [selected, setSelected] = useOperationsTab();
  const selectedView = available.find((view) => view.id === selected) ?? available[0];
  const id = useId();
  const hasSandboxes = available.some((view) => view.id === 'sandboxes');
  const prefetchDefault = hasSandboxes && selectedView?.id !== 'sandboxes';
  const prefetchRelease = useRef<(() => void) | undefined>(undefined);
  const warmDefault = () => {
    if (prefetchDefault) retainDefaultReport(prefetchRelease, client.adminUsageMetrics, principal);
  };
  useDocumentTitle('Operations');
  useEffect(() => {
    // The visible dashboard owns its selected report. Keeping a second default
    // owner here would continue polling it after the user changes filters.
    if (prefetchDefault) retainDefaultReport(prefetchRelease, client.adminUsageMetrics, principal);
    return () => {
      const release = prefetchRelease.current;
      prefetchRelease.current = undefined;
      // Let the newly mounted dashboard subscribe in this commit before
      // releasing the prefetch owner; leaving Operations still cancels it.
      queueMicrotask(() => release?.());
    };
  }, [client.adminUsageMetrics, prefetchDefault, principal]);
  if (selectedView === undefined) return null;
  const canControl = capabilities.includes('control_any_sandbox');
  return (
    <section className="operator" aria-labelledby={`${id}-heading`}>
      <div className="operator-header">
        <PageHeader
          breadcrumb={[{ label: 'Operations' }, { label: selectedView.label }]}
          title="Operations"
          headingId={`${id}-heading`}
        />
      </div>
      <div className="operator-tabs" role="tablist" aria-label="Operations views">
        {available.map((view, index) => (
          <button
            key={view.id}
            id={`${id}-${view.id}`}
            type="button"
            role="tab"
            aria-selected={selectedView.id === view.id}
            aria-controls={`${id}-panel`}
            tabIndex={selectedView.id === view.id ? 0 : -1}
            onClick={() => setSelected(view.id)}
            onMouseEnter={() => {
              if (view.id === 'sandboxes') warmDefault();
            }}
            onFocus={() => {
              if (view.id === 'sandboxes') warmDefault();
            }}
            onKeyDown={(event) => {
              const next =
                event.key === 'ArrowRight'
                  ? (index + 1) % available.length
                  : event.key === 'ArrowLeft'
                    ? (index + available.length - 1) % available.length
                    : event.key === 'Home'
                      ? 0
                      : event.key === 'End'
                        ? available.length - 1
                        : undefined;
              if (next === undefined) return;
              event.preventDefault();
              const target = available[next];
              if (target !== undefined) {
                setSelected(target.id);
                document.getElementById(`${id}-${target.id}`)?.focus();
              }
            }}
          >
            {view.label}
          </button>
        ))}
      </div>
      <div
        className="operator-scroll"
        id={`${id}-panel`}
        role="tabpanel"
        aria-labelledby={`${id}-${selectedView.id}`}
      >
        {selectedView.id === 'sandboxes' ? (
          <>
            {bindingDiagnostics && principal.principal !== undefined && (
              <BindingHolds key={principal.cacheKey} client={client} />
            )}
            <Sandboxes client={client} canControl={canControl} principal={principal} />
          </>
        ) : null}
        {selectedView.id === 'settings' ? (
          <PlatformSettings
            client={client}
            canReadAudit={capabilities.includes('read_activity_log')}
          />
        ) : null}
        {selectedView.id === 'warm_pools' ? <WarmPools client={client} /> : null}
        {selectedView.id === 'activity' ? (
          <Activity
            client={client}
            canSearchActors={capabilities.includes('read_user_directory')}
          />
        ) : null}
      </div>
    </section>
  );
}

function retainDefaultReport(
  owner: RefObject<(() => void) | undefined>,
  api: MetricsClient,
  principal: UsageReportOwner,
) {
  const release = prefetchUsageReport(api, defaultSelection, true, principal);
  // Replace ownership without cancelling a request adopted by the dashboard.
  owner.current?.();
  owner.current = release;
}
