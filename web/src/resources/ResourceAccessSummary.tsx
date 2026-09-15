import { useMemo } from 'react';
import type { EffectiveAccess } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { summarizeAccess, type AccessSummary } from './accessSummary';
import { useAuthorizedRead } from './useAuthorizedRead';
import './ResourceAccessSummary.css';

export function ResourceAccessSummary({
  value,
  className = '',
}: {
  readonly value?: AccessSummary;
  readonly className?: string;
}) {
  return (
    <span className={`resource-access-summary ${className}`.trim()}>
      Owner: {value?.owner ?? 'Unavailable'} · Your access: {value?.role ?? 'Unavailable'}
    </span>
  );
}

/** Uses the detail view's current policy proof and only authorized organization names. */
export function ResourceAccessDetails({
  client,
  access,
}: {
  readonly client: ResourceViewsClient;
  readonly access: EffectiveAccess;
}) {
  const isOrganization = access.owner?.kind === 'organization';
  const read = useMemo(
    () =>
      !isOrganization
        ? undefined
        : async (signal: AbortSignal) => ({
            value: (await client.resourceAccess.organizations({ signal })).items,
          }),
    [client, isOrganization],
  );
  const names = useAuthorizedRead(read);
  return (
    <ResourceAccessSummary
      value={summarizeAccess(access, names.state.status === 'ready' ? names.state.value : [])}
    />
  );
}
