import { useId, useState } from 'react';
import type { OperatorClient } from '../operator/client';
import type { AppRoute } from '../routing/routes';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import {
  appearanceLabel,
  APPEARANCE_ORDER,
  setAppearance,
  storedAppearance,
} from '../theme/appearance';
import { Button } from '../ui/Button';
import { SandboxUsagePanel } from './SandboxUsagePanel';
import '../operator/Operations.css';

export function Settings({
  client,
  tab,
  onNavigate,
}: {
  readonly client: OperatorClient['usage'];
  readonly tab: 'general' | 'usage';
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const id = useId();
  const [appearance, setLocalAppearance] = useState(storedAppearance);
  useDocumentTitle('Settings');
  const tabs = ['general', 'usage'] as const;
  return (
    <section className="operator" aria-label="Settings">
      <div className="operator-header">
        <PageHeader title="Settings" />
      </div>
      <div className="operator-tabs" role="tablist" aria-label="Settings tabs">
        {tabs.map((value, index) => (
          <button
            key={value}
            type="button"
            role="tab"
            id={`${id}-${value}`}
            aria-selected={value === tab}
            aria-controls={`${id}-panel`}
            tabIndex={value === tab ? 0 : -1}
            onClick={() => onNavigate({ kind: 'settings', tab: value })}
            onKeyDown={(event) => {
              const next =
                event.key === 'Home'
                  ? 0
                  : event.key === 'End'
                    ? 1
                    : event.key === 'ArrowLeft' || event.key === 'ArrowRight'
                      ? 1 - index
                      : undefined;
              const target = next === undefined ? undefined : tabs[next];
              if (!target) return;
              event.preventDefault();
              onNavigate({ kind: 'settings', tab: target });
              document.getElementById(`${id}-${target}`)?.focus();
            }}
          >
            {value === 'general' ? 'General' : 'Usage'}
          </button>
        ))}
      </div>
      <div
        className="operator-scroll"
        role="tabpanel"
        id={`${id}-panel`}
        aria-labelledby={`${id}-${tab}`}
      >
        {tab === 'usage' ? (
          <SandboxUsagePanel client={client} />
        ) : (
          <section>
            <h2>Appearance</h2>
            <p>Choose how CMA looks in this browser.</p>
            <div className="operator-actions">
              {APPEARANCE_ORDER.map((value) => (
                <Button
                  key={value}
                  aria-pressed={appearance === value}
                  onClick={() => {
                    setAppearance(value);
                    setLocalAppearance(value);
                  }}
                >
                  {appearanceLabel(value)}
                </Button>
              ))}
            </div>
          </section>
        )}
      </div>
    </section>
  );
}
