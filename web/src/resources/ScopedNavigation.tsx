import { useMemo, type ReactNode } from 'react';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { Picker } from '../ui/Picker';
import { useAuthorizedRead } from './useAuthorizedRead';
import { ResourceLink } from './ResourceLink';
import './ScopedNavigation.css';
import { scopeRoute, scopeValue } from './scope';
import { SharedAgentNavigation } from './SharedAgentNavigation';
import { SandboxTree } from './SandboxTree';

export function ScopedNavigation({
  client,
  personal,
  scope,
  onScopeChange,
  onNavigate,
}: {
  readonly client?: ResourceViewsClient;
  readonly personal: ReactNode;
  readonly scope: string;
  readonly onScopeChange: (scope: string) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const read = useMemo(
    () =>
      client === undefined
        ? undefined
        : async (signal: AbortSignal) => ({
            value: (await client.resourceAccess.organizations({ signal })).items,
          }),
    [client],
  );
  const current = useAuthorizedRead(read);
  if (client === undefined) return personal;
  const { organization, shared } = scopeRoute(scope);
  const organizations = current.state.status === 'ready' ? current.state.value : [];
  const selected = organizations.find((choice) => choice.id === organization);
  const options = [
    { value: '', label: 'Personal' },
    { value: 'shared', label: 'Shared with me' },
    ...organizations.map((choice, index) => ({
      value: scopeValue(choice.id),
      label: choice.display_name?.trim() || `Organization ${index + 1}`,
    })),
  ];
  if (organization !== undefined && selected === undefined)
    options.push({ value: scopeValue(organization), label: 'Selected organization' });
  return (
    <div className="scoped-navigation">
      <Picker label="Navigation scope" value={scope} options={options} onChange={onScopeChange} />
      {current.state.status === 'error' ? (
        <p>Organization choices are unavailable. Personal navigation remains available.</p>
      ) : null}
      {shared === true ? (
        <>
          <SandboxTree key="shared" client={client} shared onNavigate={onNavigate} />
          <SharedAgentNavigation client={client} onNavigate={onNavigate} />
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
            onNavigate={onNavigate}
          />
        </>
      )}
    </div>
  );
}
