import { useState, type ReactNode } from 'react';
import type { NavigationScopeChoices } from './NavigationScope';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { ResourceLink } from './ResourceLink';
import './ScopedNavigation.css';
import { scopeRoute } from './scope';
import { SharedAgentNavigation } from './SharedAgentNavigation';
import { SandboxTree } from './SandboxTree';
import { AccessStatusPanel } from './AccessStatusPanel';
import { Button } from '../ui/Button';

export function ScopedNavigation({
  client,
  personal,
  scope,
  choices,
  onNavigate,
  route,
  view = 'sandbox',
  statusFilter = 'all',
}: {
  readonly client?: ResourceViewsClient;
  readonly view?: 'agent' | 'sandbox';
  readonly statusFilter?: string;
  readonly route?: AppRoute;
  readonly personal: ReactNode;
  readonly scope: string;
  readonly choices: NavigationScopeChoices;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [accessOpen, setAccessOpen] = useState(false);
  const [accessReviewed, setAccessReviewed] = useState(false);
  if (client === undefined) return personal;
  const { organization, shared } = scopeRoute(scope);
  const { selected, unavailable } = choices;
  return (
    <div className="scoped-navigation">
      {unavailable || accessReviewed ? (
        <div className="scoped-navigation__access">
          {unavailable ? (
            <p>Organization choices are unavailable. Personal navigation remains available.</p>
          ) : null}
          <Button
            size="sm"
            onClick={() => {
              setAccessReviewed(true);
              setAccessOpen(true);
            }}
          >
            Review access
          </Button>
        </div>
      ) : null}
      {accessOpen ? (
        <AccessStatusPanel choices={choices} scope={scope} onClose={() => setAccessOpen(false)} />
      ) : null}
      {view === 'agent' && (shared || organization) ? (
        <SharedAgentNavigation
          key={`${scope}:${statusFilter}`}
          client={client}
          route={route}
          organization={organization}
          shared={shared === true}
          statusFilter={statusFilter}
          onNavigate={onNavigate}
        />
      ) : shared === true ? (
        <SandboxTree
          statusFilter={statusFilter}
          key="shared"
          client={client}
          route={route}
          shared
          onNavigate={onNavigate}
        />
      ) : organization === undefined ? (
        personal
      ) : (
        <>
          <p>
            {selected?.display_name?.trim() || 'Organization'}
            {selected === undefined ? '' : ` · Organization ${selected.role}`}
          </p>
          <ResourceLink route={{ kind: 'inventory', organization }} onNavigate={onNavigate}>
            All authorized Agents
          </ResourceLink>
          <SandboxTree
            statusFilter={statusFilter}
            key={organization}
            client={client}
            organization={organization}
            route={route}
            onNavigate={onNavigate}
          />
        </>
      )}
    </div>
  );
}
