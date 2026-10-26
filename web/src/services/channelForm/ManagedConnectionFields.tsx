import { useState } from 'react';
import { Button } from '../../ui/Button';
import { ServiceError, serviceRequest, type Denied } from '../client';
import { AdmissionFields } from './AdmissionFields';
import { Choice, Section, Toggle } from './Fields';
import { defaultSource, optional, type Fields } from './model';
interface Connection {
  connection_ref: string;
  status: string;
  label?: string | null;
}
interface Page {
  items: Connection[];
  next_after?: string | null;
}

/** Existing CMA-owned Telegram connections only; does not create upstream bots. */
export function ManagedConnectionFields({
  value,
  onChange,
  denied,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
  denied: Denied;
}) {
  const [enabled, setEnabled] = useState(Boolean(value.connection_ref));
  const [manualSources, setManualSources] = useState(value.sources);
  const [savedRef, setSavedRef] = useState(value.connection_ref);
  const [savedAdmission, setSavedAdmission] = useState(value.connection_admission);
  const [items, setItems] = useState<Connection[]>();
  const [cursor, setCursor] = useState<string | null>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  async function load(after?: string) {
    setBusy(true);
    setError(undefined);
    try {
      const page = await serviceRequest<Page>(
        'event-gateway',
        `/v1/channel-connections${after ? `?after=${encodeURIComponent(after)}` : ''}`,
      );
      setItems((old) => (after ? [...(old ?? []), ...page.items] : page.items));
      setCursor(page.next_after);
    } catch (error) {
      if (error instanceof ServiceError && [401, 403].includes(error.status)) denied(error.status);
      setError(error instanceof Error ? error.message : 'Connections could not be loaded.');
    } finally {
      setBusy(false);
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
            if (items === undefined && !busy) void load();
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
              ...(items ?? []).map((item) => ({
                value: item.connection_ref,
                label: item.label || item.connection_ref,
                ...(item.status !== 'ready'
                  ? { unavailable: item.status.replaceAll('_', ' ') }
                  : {}),
              })),
            ]}
            onChange={(connection_ref) => onChange({ ...value, connection_ref })}
          />
          {busy ? <p role="status">Loading your connections…</p> : null}
          {items?.length === 0 ? (
            <p className="channel-field-hint">
              No saved CMA connections. Switch this option off to configure a NyxID relay or direct
              Telegram bot.
            </p>
          ) : null}
          {error ? (
            <p role="alert">
              {error}{' '}
              <Button disabled={busy} onClick={() => void load()}>
                Retry connections
              </Button>
            </p>
          ) : null}
          {cursor ? (
            <Button disabled={busy} onClick={() => void load(cursor)}>
              Load more connections
            </Button>
          ) : null}
          <AdmissionFields
            label="Connection admission"
            value={value.connection_admission}
            onChange={(connection_admission) => onChange({ ...value, connection_admission })}
          />
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
