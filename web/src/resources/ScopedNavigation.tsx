import type { ReactNode } from 'react';
import type { NavigationScopeChoices } from './NavigationScope';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { ResourceLink } from './ResourceLink';
import './ScopedNavigation.css';
import { scopeRoute } from './scope';
import { SharedAgentNavigation } from './SharedAgentNavigation';
import { SandboxTree } from './SandboxTree';

export function ScopedNavigation({
  client,
  personal,
  scope,
  choices,
  onNavigate,
  route,
}: {
  readonly client?: ResourceViewsClient;
  readonly route?: AppRoute;
  readonly personal: ReactNode;
  readonly scope: string;
  readonly choices: NavigationScopeChoices;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  if (client === undefined) return personal;
  const { organization, shared } = scopeRoute(scope);
  const { selected, unavailable } = choices;
  return (
    <div className="scoped-navigation">
      {unavailable ? (
        <p>Organization choices are unavailable. Personal navigation remains available.</p>
      ) : null}
      {shared === true ? (
        <>
          <SandboxTree key="shared" client={client} route={route} shared onNavigate={onNavigate} />
          <SharedAgentNavigation client={client} route={route} onNavigate={onNavigate} />
        </>
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
