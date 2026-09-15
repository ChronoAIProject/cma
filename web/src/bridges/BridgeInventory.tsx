import { useMemo, useState } from 'react';
import type { BridgePageProps } from './BridgePage';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { ResourceLink } from '../resources/ResourceLink';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';
import { scopeRoute, scopeValue } from '../resources/scope';

export function BridgeInventory({
  client,
  organization,
  onNavigate,
}: BridgePageProps & { readonly organization?: string }) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => async (signal: AbortSignal) => ({
      value: await client.bridges.list({ organization, cursor, limit: 50, signal }),
    }),
    [client, organization, cursor],
  );
  const inventory = useAuthorizedRead(read);
  const readScopes = useMemo(
    () => async (signal: AbortSignal) => ({
      value: (await client.resourceAccess.organizations({ signal })).items,
    }),
    [client],
  );
  const scopes = useAuthorizedRead(readScopes);
  const organizations = scopes.state.status === 'ready' ? scopes.state.value : [];
  const selected = organizations.find((item) => item.id === organization);
  const choices = [
    { value: '', label: 'Personal' },
    ...organizations.map((item, i) => ({
      value: scopeValue(item.id),
      label: item.display_name?.trim() || `Organization ${i + 1}`,
    })),
  ];
  if (organization !== undefined && selected === undefined)
    choices.push({ value: scopeValue(organization), label: 'Selected organization' });
  const page = inventory.state.status === 'ready' ? inventory.state.value : undefined;
  return (
    <section aria-labelledby="bridges-title">
      <header className="bridges__header">
        <h1 id="bridges-title">Bridges</h1>
        <p>Connect an existing channel bot to an Agent conversation in CMA.</p>
        <div className="bridges__actions">
          <Picker
            label="Bridge owner scope"
            value={scopeValue(organization)}
            options={choices}
            onChange={(value) =>
              onNavigate({ kind: 'bridges', organization: scopeRoute(value).organization })
            }
          />
          <Button onClick={inventory.reload}>Refresh bridges</Button>
          <Button
            variant="primary"
            disabled={organization !== undefined && selected?.role !== 'admin'}
            onClick={() => onNavigate({ kind: 'bridge_new', organization })}
          >
            New bridge
          </Button>
        </div>
        {organization !== undefined && selected?.role !== 'admin' ? (
          <p>Organization admins manage bridges. Current members can view their status.</p>
        ) : null}
        {scopes.state.status === 'error' ? (
          <p role="status">
            Organization choices are unavailable.{' '}
            <Button onClick={scopes.reload}>Retry choices</Button>
          </p>
        ) : null}
      </header>
      <div className="bridges__body">
        {inventory.state.status === 'loading' ? <p role="status">Loading bridges</p> : null}
        {inventory.state.status === 'error' ? (
          <p role="alert">Bridges could not be loaded. Refresh to check current access.</p>
        ) : null}
        {page === undefined ? null : (
          <>
            {page.items.length === 0 ? (
              <p>No bridges in this scope.</p>
            ) : (
              <ul className="bridges__rows" aria-label="Bridges in this scope">
                {page.items.map((item) => (
                  <li key={item.id}>
                    <ResourceLink
                      route={{ kind: 'bridge', bridgeId: item.id }}
                      onNavigate={onNavigate}
                    >
                      {item.display_name}
                    </ResourceLink>
                    <span>{item.platform}</span>
                    <span>{item.state.kind}</span>
                    <span>{item.can_manage ? 'Management available' : 'View only'}</span>
                  </li>
                ))}
              </ul>
            )}
            <nav className="bridges__actions" aria-label="Bridge inventory pages">
              <Button
                disabled={cursors.length === 1}
                onClick={() => setCursors((values) => values.slice(0, -1))}
              >
                Previous page
              </Button>
              <span>Page {cursors.length}</span>
              <Button
                disabled={page.next_cursor === undefined}
                onClick={() => setCursors((values) => [...values, page.next_cursor])}
              >
                Next page
              </Button>
            </nav>
          </>
        )}
      </div>
    </section>
  );
}
