import { useLayoutEffect, useRef, useState } from 'react';
import {
  ConnectionPageSchema,
  type ConnectionView,
} from '../../api-client/cmaeg/types/connections';
import { Button } from '../../ui/Button';
import { ServiceError, serviceRequest, type Denied } from '../client';
import { AdmissionFields } from './AdmissionFields';
import { Choice, Section, Toggle } from './Fields';
import { defaultSource, optional, type Fields } from './model';

type ConnectionList = { items: readonly ConnectionView[] } & (
  | { status: 'idle' | 'denied' }
  | { status: 'loaded'; next?: string }
  | { status: 'failed'; after?: string }
);

/** Existing CMA-owned Telegram connections only; does not create upstream bots. */
export function ManagedConnectionFields({
  value,
  active = true,
  onChange,
  hideAdmission = false,
  denied,
}: {
  active?: boolean;
  hideAdmission?: boolean;
  value: Fields;
  onChange: (value: Fields) => void;
  denied: Denied;
}) {
  const [enabled, setEnabled] = useState(Boolean(value.connection_ref));
  const [manualSources, setManualSources] = useState(value.sources);
  const [savedRef, setSavedRef] = useState(value.connection_ref);
  const [savedAdmission, setSavedAdmission] = useState(value.connection_admission);
  const [list, setList] = useState<ConnectionList>({ status: 'idle', items: [] });
  const [loading, setLoading] = useState(false);
  const pending = useRef<AbortController | undefined>(undefined);

  if (!active && loading) setLoading(false);
  // End the read when the retained editor hides or its owner changes.
  useLayoutEffect(
    () => () => {
      const request = pending.current;
      pending.current = undefined;
      request?.abort();
    },
    [active],
  );

  async function loadConnections(after?: string) {
    if (!active || pending.current) return;
    const request = new AbortController();
    pending.current = request;
    const signal = AbortSignal.any([request.signal, AbortSignal.timeout(15000)]);
    setLoading(true);
    try {
      const response = await serviceRequest<unknown>(
        'event-gateway',
        `/v1/channel-connections${after === undefined ? '' : `?after=${encodeURIComponent(after)}`}`,
        { signal },
      );
      if (pending.current !== request) return;
      signal.throwIfAborted();
      const page = ConnectionPageSchema(response);
      const items = new Map(list.items.map((item) => [item.connection_ref, item]));
      for (const item of page.items) items.set(item.connection_ref, item);
      setList({ status: 'loaded', items: [...items.values()], next: page.next_after ?? undefined });
    } catch (error) {
      if (pending.current !== request) return;
      if (error instanceof ServiceError && (error.status === 401 || error.status === 403)) {
        setList({ status: 'denied', items: [] });
        denied(error.status);
      } else {
        setList({ status: 'failed', items: list.items, after });
      }
    } finally {
      if (pending.current === request) {
        pending.current = undefined;
        setLoading(false);
      }
    }
  }
  return (
    <Section title="Connection setup">
      <Toggle
        label="Use an existing CMA-managed Telegram connection"
        value={enabled}
        onChange={(next) => {
          setEnabled(next);
          if (next) {
            setManualSources(value.sources);
            onChange({
              ...value,
              sources: [],
              connection_ref: savedRef ?? '',
              connection_admission: savedAdmission ?? { type: 'allowlist', sender_ids: [] },
            });
            if (list.status === 'idle') void loadConnections();
          } else {
            setSavedRef(value.connection_ref);
            setSavedAdmission(value.connection_admission);
            onChange(
              optional(
                optional(
                  { ...value, sources: manualSources ?? [defaultSource('nyxid_relay')] },
                  'connection_ref',
                  undefined,
                ),
                'connection_admission',
                undefined,
              ),
            );
          }
        }}
      />
      {enabled ? (
        <>
          <p className="channel-field-hint">
            Choose a ready, unbound Telegram connection owned by this account. Manual producer
            settings and credentials remain in your draft while this option is selected. Connection
            creation requires an admitted originating event; this form does not create bots. A
            connection already attached to another channel will be refused on save.
          </p>
          <Choice
            label="Saved Telegram connection"
            value={value.connection_ref}
            options={[
              {
                value: '',
                label: 'Choose a connection',
                unavailable: 'Select a ready connection.',
              },
              ...list.items.map((item) => ({
                value: item.connection_ref,
                label: item.label || item.connection_ref,
                ...(item.status !== 'ready'
                  ? { unavailable: item.status.replaceAll('_', ' ') }
                  : {}),
              })),
            ]}
            onChange={(connection_ref) => onChange({ ...value, connection_ref })}
          />
          {loading ? <p role="status">Loading your connections…</p> : null}
          {!loading && list.status === 'idle' ? (
            <Button onClick={() => void loadConnections()}>Load connections</Button>
          ) : null}
          {!loading &&
          list.status === 'loaded' &&
          list.next === undefined &&
          list.items.every((item) => item.status !== 'ready') ? (
            <p className="channel-field-hint">
              No saved CMA connections are available. Switch this option off to configure a NyxID
              relay or direct Telegram bot.
            </p>
          ) : null}
          {!loading && list.status === 'failed' ? (
            <p role="alert">
              Saved Telegram connections could not be loaded. Your draft is retained. You can retry
              or switch this option off for a NyxID relay or direct Telegram bot.{' '}
              <Button onClick={() => void loadConnections(list.after)}>Retry connections</Button>
            </p>
          ) : null}
          {!loading && list.status === 'loaded' && list.next !== undefined ? (
            <Button onClick={() => void loadConnections(list.next)}>Load more connections</Button>
          ) : null}
          {!hideAdmission ? (
            <AdmissionFields
              path="/connection_admission"
              label="Connection admission"
              value={value.connection_admission}
              onChange={(connection_admission) => onChange({ ...value, connection_admission })}
            />
          ) : null}
        </>
      ) : (
        <p className="channel-field-hint">
          Configure a NyxID relay or direct bot below. NyxID route IDs and CMA-managed connection
          references are separate identifiers.
        </p>
      )}
    </Section>
  );
}
