import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { cmaeg } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
import { serviceRequest, ServiceError } from '../client';
import { Choice, Toggle } from './Fields';

type Discovery =
  | { status: 'idle' | 'loading' }
  | { status: 'ready'; items: readonly cmaeg.ResourceConnection[] }
  | { status: 'failed'; message: string };

/** Mounted within the owner/permission-keyed Event Gateway and region-keyed form. */
export function LarkConnectionSelector({
  platform,
  connectionId,
  catalogId,
  onSelect,
  children,
}: {
  platform: cmaeg.ResourcePlatform;
  connectionId: string;
  catalogId: string;
  onSelect: (connection: cmaeg.ResourceConnection) => void;
  children: ReactNode;
}) {
  const [discovery, setDiscovery] = useState<Discovery>({ status: 'idle' });
  const [manual, setManual] = useState(connectionId !== '' || catalogId !== '');
  const pending = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => pending.current?.abort(), []);
  const load = async () => {
    pending.current?.abort();
    const request = new AbortController();
    pending.current = request;
    setDiscovery({ status: 'loading' });
    try {
      const { items } = await serviceRequest<cmaeg.ResourceConnectionList>(
        'event-gateway',
        `/v1/relay-resource-connections?platform=${platform}`,
        { signal: AbortSignal.any([request.signal, AbortSignal.timeout(15000)]) },
      );
      if (
        !Array.isArray(items) ||
        items.length > 1000 ||
        items.some(
          (item) =>
            item === null ||
            typeof item !== 'object' ||
            item.platform !== platform ||
            !['personal', 'organization'].includes(item.source) ||
            typeof item.user_service_id !== 'string' ||
            typeof item.catalog_service_id !== 'string' ||
            !/^[a-zA-Z0-9_-]{1,128}$/.test(item.user_service_id) ||
            !/^[a-zA-Z0-9_-]{1,128}$/.test(item.catalog_service_id) ||
            typeof item.name !== 'string' ||
            item.name.trim() === '' ||
            item.name.length > 256,
        ) ||
        new Set(items.map((item) => item.user_service_id)).size !== items.length
      )
        throw new Error('Invalid connection inventory');
      if (!request.signal.aborted) setDiscovery({ status: 'ready', items });
    } catch (error) {
      if (!request.signal.aborted)
        setDiscovery({
          status: 'failed',
          message:
            error instanceof ServiceError && error.status === 404
              ? 'Connection selection is not available on this gateway yet. You can enter the existing IDs manually.'
              : error instanceof ServiceError && error.status === 401
                ? 'Sign in again to load your connections. Your entered binding is retained.'
                : error instanceof ServiceError && error.status === 403
                  ? 'Your current access does not allow connection discovery. You can keep an existing binding.'
                  : 'Connections could not be loaded. Retry or enter the existing IDs manually.',
        });
    }
  };
  const items = discovery.status === 'ready' ? discovery.items : [];
  const selected = items.find(
    (item) => item.user_service_id === connectionId && item.catalog_service_id === catalogId,
  );
  return (
    <>
      <div className="channel-form-row">
        <Button disabled={discovery.status === 'loading'} onClick={() => void load()}>
          {discovery.status === 'failed'
            ? 'Retry connections'
            : discovery.status === 'ready'
              ? 'Refresh connections'
              : 'Load my bot connections'}
        </Button>
      </div>
      {discovery.status === 'loading' ? (
        <p className="channel-field-hint" role="status">
          Loading your existing connections…
        </p>
      ) : null}
      {discovery.status === 'failed' ? (
        <p className="channel-field-hint" role="status">
          {discovery.message}
        </p>
      ) : null}
      {discovery.status === 'ready' && items.length > 0 ? (
        <Choice
          label="Existing bot API connection"
          value={selected?.user_service_id ?? ''}
          options={[
            {
              value: '',
              label: connectionId ? 'Keep current binding' : 'Choose a connection',
              unavailable: 'Select an existing connection below.',
            },
            ...items.map((item) => ({
              value: item.user_service_id,
              label: `${item.name} · ${item.source === 'organization' ? 'Organization' : 'Personal'} · ${item.user_service_id}`,
            })),
          ]}
          onChange={(id) => {
            const connection = items.find((item) => item.user_service_id === id);
            if (connection) {
              onSelect(connection);
              setManual(false);
            }
          }}
        />
      ) : null}
      {discovery.status === 'ready' && items.length === 0 ? (
        <p className="channel-field-hint" role="status">
          No eligible {platform === 'feishu' ? 'Feishu' : 'Lark'} bot connections were found. Some
          shared custom connections require manual IDs; existing bindings are retained.
        </p>
      ) : null}
      {discovery.status === 'ready' && connectionId && !selected ? (
        <p className="channel-field-hint">
          Your current binding is retained even though it is not in this connection list.
        </p>
      ) : null}
      <p className="channel-field-hint">
        Choosing a connection does not grant access to the channel key or confirm the receiving app
        and bot.
      </p>
      <Toggle label="Enter connection IDs manually" value={manual} onChange={setManual} />
      {manual ? children : null}
    </>
  );
}
