import { useId } from 'react';
import { useOperationsTab } from './useOperationsTab';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import type { Capability } from '../auth/types';
import type { OperatorClient } from './client';
import { Sandboxes } from './Sandboxes';
import { PlatformSettings } from './PlatformSettings';
import { WarmPools } from './WarmPools';
import { Activity } from './Activity';
import { operatorViews } from './capabilities';
import './Operations.css';

export { canOpenOperations } from './capabilities';

export function Operations({
  client,
  capabilities,
}: {
  readonly client: OperatorClient;
  readonly capabilities: readonly Capability[];
}) {
  const available = operatorViews.filter((view) => capabilities.includes(view.capability));
  const [selected, setSelected] = useOperationsTab();
  const selectedView = available.find((view) => view.id === selected) ?? available[0];
  const id = useId();
  useDocumentTitle('Operations');
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
          <Sandboxes client={client} canControl={canControl} />
        ) : null}
        {selectedView.id === 'settings' ? <PlatformSettings client={client} /> : null}
        {selectedView.id === 'warm_pools' ? <WarmPools client={client} /> : null}
        {selectedView.id === 'activity' ? <Activity client={client} /> : null}
      </div>
    </section>
  );
}
