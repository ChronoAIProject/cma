import { useState } from 'react';
import { Button } from '../ui/Button';
import { segment, ServiceError, serviceRequest, type Denied } from './client';
import { useRelayCapabilities } from './RelayCapabilities';
import { validateRelayAuthoring } from './channelForm/relayAuthoring';
import { NodeFields, nodeCredentials } from './channelForm/NodeFields';
import {
  ConsumerPolicyFields,
  initialRouting,
  RoutingFields,
  routingRequest,
} from './channelForm/RoutingFields';
import {
  channelDraft,
  channelCredentials,
  channelRequest,
  confirmedDraft,
  object,
  records,
  selectedDraft,
  type EditorSection,
  type Fields,
} from './channelForm/model';
export type { EditorSection } from './channelForm/model';
export interface Channel {
  native_discord_activation?: {
    state: string;
    detail: string;
    capacity: number;
    selected_records: number;
  };
  created_at?: string;
  owner?: unknown;
  channel_id: string;
  record_id: string;
  version: number;
  provider: { slug: string; level: string };
  definition: {
    [key: string]: unknown;
    name: string;
    sources: unknown[];
    schema_version?: number;
    profile?: unknown;
    provider?: {
      [key: string]: unknown;
      slug?: string;
      binding_id?: string;
      target?: { type: 'profile' } | { type: 'existing_agent'; agent_id: string };
    };
    partition?: unknown;
    reply?: unknown;
    routing?: unknown;
    connection_ref?: string;
  };
  endpoints: {
    custom_event_url: string;
    nyxid_callback_url: string;
    telegram_webhook_url?: string;
    whatsapp_webhook_url?: string;
    native_lark_webhook_url?: string;
  };
}
export type ChannelListItem = Omit<Channel, 'channel_id' | 'endpoints'> & {
  readonly channel_id?: string | null;
  readonly endpoints?: Channel['endpoints'] | null;
};
export function hasChannelAddress(channel: ChannelListItem): channel is Channel {
  return (
    typeof channel.channel_id === 'string' &&
    channel.channel_id.length > 0 &&
    channel.endpoints != null &&
    typeof channel.endpoints.custom_event_url === 'string' &&
    typeof channel.endpoints.nyxid_callback_url === 'string'
  );
}
interface Pending {
  key: string;
  body: string;
  record: string;
  path: string;
  method: 'POST' | 'PUT';
  section: EditorSection;
  kind: 'channel' | 'routing';
  credentialKeys: readonly string[];
}
export function ChannelEditor({
  channel,
  section = { kind: 'channel' },
  ownerSubject = '',
  onDone,
  denied,
}: {
  readonly channel?: Channel;
  readonly section?: EditorSection;
  readonly ownerSubject?: string;
  readonly onDone: (channel?: Channel) => void;
  readonly denied: Denied;
}) {
  const discovery = useRelayCapabilities();
  const [baseline, setBaseline] = useState(channel);
  const [draft, setDraft] = useState(() => channelDraft(channel, ownerSubject));
  const [routing, setRouting] = useState(() =>
    channel?.definition.routing ? object(channel.definition.routing) : initialRouting(),
  );
  const [credentials, setCredentials] = useState<Fields>({});
  const [pending, setPending] = useState<Pending>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [busy, setBusy] = useState(false);
  const create = !baseline;
  const additional = section.kind === 'consumer' && section.id !== 'default';
  async function send(request: Pending) {
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    try {
      const value = await serviceRequest<ChannelListItem>('event-gateway', request.path, {
        method: request.method,
        body: request.body,
        headers: { 'Idempotency-Key': request.key },
      });
      setPending(undefined);
      if (hasChannelAddress(value)) {
        setBaseline(value);
        if (request.kind === 'channel') {
          setDraft((current) => confirmedDraft(current, channelDraft(value), request.section));
          setCredentials((current) =>
            Object.fromEntries(
              Object.entries(current).filter(([key]) => !request.credentialKeys.includes(key)),
            ),
          );
        } else {
          const confirmed = object(value.definition.routing);
          setRouting((current) => routingProjection(current, confirmed, request.section));
        }
      }
      setNotice(
        value.native_discord_activation
          ? `Changes saved. ${value.native_discord_activation.detail}`
          : 'Changes saved.',
      );
      onDone(hasChannelAddress(value) ? value : undefined);
    } catch (e) {
      if (e instanceof ServiceError && [401, 403].includes(e.status)) denied(e.status);
      if (e instanceof ServiceError && e.status === 400) setPending(undefined);
      setError(
        e instanceof Error
          ? e.message
          : 'The result is unknown. Retry the same request to confirm it.',
      );
    } finally {
      setBusy(false);
    }
  }
  function submit(kind: 'channel' | 'routing') {
    if (pending) {
      void send(pending);
      return;
    }
    try {
      const record = baseline?.record_id ?? crypto.randomUUID();
      const projected = baseline ? selectedDraft(channelDraft(baseline), draft, section) : draft;
      const selectedCredentials = baseline
        ? nodeCredentials(credentials, section, projected)
        : channelCredentials(credentials, projected, true);
      if (kind === 'channel')
        validateRelayAuthoring(
          discovery,
          projected,
          baseline ? channelDraft(baseline) : undefined,
          selectedCredentials,
        );
      const body =
        kind === 'routing'
          ? {
              expected_version: baseline?.version,
              routing: routingRequest(
                routingProjection(
                  baseline?.definition.routing
                    ? object(baseline.definition.routing)
                    : initialRouting(),
                  routing,
                  section,
                ),
              ),
            }
          : channelRequest(projected, selectedCredentials, baseline, record);
      const request: Pending = {
        key: crypto.randomUUID(),
        body: JSON.stringify(body),
        record,
        path: baseline
          ? `/v1/channels/${segment(baseline.channel_id)}${kind === 'routing' ? '/routing' : ''}`
          : '/v1/channels',
        method: baseline ? 'PUT' : 'POST',
        section,
        kind,
        credentialKeys: Object.keys(object(body.credentials)).flatMap((key) =>
          key === 'hmac_secrets' ? ['hmac_secrets', 'replace_hmac'] : [key],
        ),
      };
      setPending(request);
      void send(request);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Review the channel fields.');
    }
  }
  async function recover() {
    if (!pending) return;
    setBusy(true);
    try {
      const current = await serviceRequest<Channel>(
        'event-gateway',
        `/v1/channel-records/${segment(pending.record)}`,
      );
      setError(
        baseline && current.version > baseline.version
          ? 'The channel has changed. Keep this draft and retry the original request to confirm its result.'
          : 'The original record was found. Retry the same request to confirm completion.',
      );
    } catch (e) {
      if (e instanceof ServiceError && [401, 403].includes(e.status)) denied(e.status);
      setError(
        e instanceof Error ? e.message : 'Recovery is unavailable. Keep this request for retry.',
      );
    } finally {
      setBusy(false);
    }
  }
  const activeConsumer =
    section.kind === 'consumer'
      ? section.id === 'default'
        ? object(routing.default_consumer)
        : records(routing.consumers).find((v) => v.id === section.id)
      : undefined;
  return (
    <div className="channel-node-editor">
      {!additional ? (
        <form
          className="service-form"
          onSubmit={(e) => {
            e.preventDefault();
            submit('channel');
          }}
        >
          <fieldset className="channel-form-section" disabled={pending !== undefined || busy}>
            <NodeFields
              draft={draft}
              onChange={setDraft}
              credentials={credentials}
              onCredentials={setCredentials}
              section={section}
              create={create}
              sourceCount={baseline?.definition.sources.length ?? 0}
              denied={denied}
            />
          </fieldset>
          {!pending ? (
            <Button type="submit" variant="primary" disabled={busy}>
              {create
                ? 'Create channel'
                : section.kind === 'producer'
                  ? 'Save producer'
                  : section.kind === 'consumer'
                    ? 'Save agent settings'
                    : 'Save channel'}
            </Button>
          ) : null}
        </form>
      ) : null}
      {!create && (section.kind === 'channel' || section.kind === 'consumer') ? (
        <form
          className="service-form"
          onSubmit={(e) => {
            e.preventDefault();
            submit('routing');
          }}
        >
          <fieldset className="channel-form-section" disabled={pending !== undefined || busy}>
            {section.kind === 'channel' ? (
              <RoutingFields value={routing} onChange={setRouting} />
            ) : activeConsumer ? (
              <>
                {additional ? (
                  <p className="channel-field-hint">
                    Provider: {String(object(activeConsumer.provider).slug ?? 'unavailable')}. Agent
                    destination and credentials remain attached to this consumer.
                  </p>
                ) : null}
                <ConsumerPolicyFields
                  value={activeConsumer}
                  onChange={(next) =>
                    setRouting((current) =>
                      section.id === 'default'
                        ? { ...current, default_consumer: next }
                        : {
                            ...current,
                            consumers: records(current.consumers).map((v) =>
                              v.id === section.id ? next : v,
                            ),
                          },
                    )
                  }
                />
              </>
            ) : (
              <p>This consumer is no longer attached. Select another node.</p>
            )}
          </fieldset>
          {!pending ? (
            <Button
              type="submit"
              disabled={busy || (section.kind === 'consumer' && !activeConsumer)}
            >
              {section.kind === 'channel' ? 'Save routing' : 'Save consumer policy'}
            </Button>
          ) : null}
        </form>
      ) : null}
      {error ? <p role="alert">{error}</p> : null}
      {notice ? (
        <p role="status">{notice}</p>
      ) : baseline?.native_discord_activation ? (
        <p role="status">{baseline.native_discord_activation.detail}</p>
      ) : null}
      {pending ? (
        <div className="service-actions">
          <Button variant="primary" disabled={busy} onClick={() => void send(pending)}>
            {busy ? 'Submitting…' : 'Retry same request'}
          </Button>
          <Button disabled={busy} onClick={() => void recover()}>
            Check original channel
          </Button>
        </div>
      ) : null}
    </div>
  );
}
function routingProjection(base: Fields, draft: Fields, section: EditorSection): Fields {
  if (section.kind === 'channel')
    return { ...draft, default_consumer: base.default_consumer, consumers: base.consumers };
  if (section.kind !== 'consumer') return base;
  if (section.id === 'default') return { ...base, default_consumer: draft.default_consumer };
  const next = records(draft.consumers).find((v) => v.id === section.id);
  return {
    ...base,
    consumers: records(base.consumers).map((v) => (v.id === section.id && next ? next : v)),
  };
}
export function DeleteChannel({
  channel,
  onDone,
  denied,
}: {
  readonly channel: Channel;
  readonly onDone: () => void;
  readonly denied: Denied;
}) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  return (
    <div>
      {confirm ? (
        <>
          <p>
            Delete {channel.definition.name}? This disconnects its provider and stops incoming event
            delivery.
          </p>
          <Button
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void serviceRequest(
                'event-gateway',
                `/v1/channels/${segment(channel.channel_id)}?expected_version=${channel.version}`,
                { method: 'DELETE' },
              )
                .then(onDone)
                .catch((e: unknown) => {
                  if (e instanceof ServiceError && [401, 403].includes(e.status)) denied(e.status);
                  setError(
                    e instanceof Error
                      ? e.message
                      : 'Deletion is unconfirmed. Refresh the channel before retrying.',
                  );
                })
                .finally(() => setBusy(false));
            }}
          >
            {busy ? 'Deleting…' : 'Confirm delete'}
          </Button>{' '}
          <Button disabled={busy} onClick={() => setConfirm(false)}>
            Cancel
          </Button>
        </>
      ) : (
        <Button onClick={() => setConfirm(true)}>Delete channel</Button>
      )}
      {error ? <p role="alert">{error}</p> : null}
    </div>
  );
}
