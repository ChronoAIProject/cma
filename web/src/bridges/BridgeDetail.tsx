import { useMemo } from 'react';
import type { BridgePageProps } from './BridgePage';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { ResourceLink } from '../resources/ResourceLink';
import { Button } from '../ui/Button';
import { BridgeManager } from './BridgeManager';
import { BridgeDeliveries } from './BridgeDeliveries';

export function BridgeDetail({
  client,
  id,
  onNavigate,
}: BridgePageProps & { readonly id: string }) {
  const read = useMemo(
    () => async (signal: AbortSignal) => ({ value: await client.bridges.read(id, { signal }) }),
    [client, id],
  );
  const current = useAuthorizedRead(read);
  const bridge = current.state.status === 'ready' ? current.state.value : undefined;
  const organization = bridge?.owner.kind === 'organization' ? bridge.owner.subject : undefined;
  return (
    <section aria-labelledby="bridge-detail-title">
      <header className="bridges__header">
        <ResourceLink route={{ kind: 'bridges', organization }} onNavigate={onNavigate}>
          Back to bridges
        </ResourceLink>
        <h1 id="bridge-detail-title">{bridge?.display_name ?? 'Bridge'}</h1>
        {bridge === undefined ? null : (
          <p>
            {bridge.platform} · {bridge.state.kind} ·{' '}
            {bridge.can_manage ? 'Management available' : 'View only'}
          </p>
        )}
        <Button onClick={current.reload}>Refresh bridge</Button>
      </header>
      <div className="bridges__body">
        {current.state.status === 'loading' ? <p role="status">Loading bridge</p> : null}
        {current.state.status === 'error' ? (
          <p role="alert">This bridge is unavailable. Refresh to check current access.</p>
        ) : null}
        {bridge === undefined ? null : (
          <p>Message processing and channel reply delivery are not available on this build.</p>
        )}
        {bridge !== undefined && !bridge.can_manage ? (
          <p>
            The owner or an organization admin can manage this bridge’s settings and connection.
          </p>
        ) : null}
        <BridgeManager
          client={client}
          id={id}
          allowed={bridge?.can_manage === true}
          onChanged={current.reload}
        />
        {bridge === undefined ? null : (
          <BridgeDeliveries
            key={id}
            client={client}
            id={id}
            revision={bridge.revision}
            allowed={bridge.can_manage}
            onAccessChanged={current.reload}
          />
        )}
      </div>
    </section>
  );
}
