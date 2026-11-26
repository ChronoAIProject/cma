import { channelResponseMatches } from './channelEvidence';
import { useChannelWriteGuard } from './useChannelWriteGuard';
import { HmacCredentialsScope } from './channelForm/HmacCredentialsScope';
import { ValidationMessageScope } from './channelForm/ValidationPath';
import { useDraftReloadWarning } from './channelForm/useDraftReloadWarning';
import {
  clearNewChannelDraft,
  releaseNewChannelDraft,
  writeNewChannelDraft,
  type NewChannelDraft,
} from './channelForm/newChannelDraft';
import { validateConnectorFields } from './channelForm/connectorValidation';
import { channelNameRequired } from './channelForm/ChannelPolicyFields';
import { validateReplyFields } from './channelForm/replyValidation';
import { revealInvalidField } from './channelForm/AdvancedFields';
import { ValidationError } from './channelForm/ValidationError';
import { useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { segment, ServiceError, serviceRequest, type Denied } from './client';
import { useRelayCapabilities } from './RelayCapabilities';
import { validateRelayAuthoring } from './channelForm/relayAuthoring';
import { validateAuthoringRules } from './channelForm/authoringRules';
import { NodeFields, nodeCredentials } from './channelForm/NodeFields';
import {
  ConsumerPolicyFields,
  initialRouting,
  RoutingFields,
  routingLimitErrors,
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
  text,
  type EditorSection,
  type Fields,
} from './channelForm/model';
import { customExecutionRequest, customExecutionWaiting } from './channelForm/customExecution';
export type { EditorSection } from './channelForm/model';
import { hasChannelAddress, type Channel, type ChannelListItem } from './channelTypes';
export { hasChannelAddress, type Channel, type ChannelListItem } from './channelTypes';
interface Pending {
  uncertain?: boolean;
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
  active = true,
  writable = true,
  section = { kind: 'channel' },
  ownerSubject = '',
  recoverDraft = false,
  initialDraft,
  onDone,
  denied,
}: {
  readonly channel?: Channel;
  readonly active?: boolean;
  readonly writable?: boolean;
  readonly section?: EditorSection;
  readonly ownerSubject?: string;
  readonly recoverDraft?: boolean;
  readonly initialDraft?: NewChannelDraft;
  readonly onDone: (channel?: Channel) => void;
  readonly denied: Denied;
}) {
  const editor = useRef<HTMLDivElement>(null);
  const discovery = useRelayCapabilities();
  const [baseline, setBaseline] = useState(channel);
  const [draft, setDraft] = useState(
    () => initialDraft?.fields ?? channelDraft(channel, ownerSubject),
  );
  const [routing, setRouting] = useState(() =>
    channel?.definition.routing ? object(channel.definition.routing) : initialRouting(),
  );
  const [credentials, setCredentials] = useState<Fields>({});
  const [pending, setPending] = useState<Pending>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [edited, setEdited] = useState(false);
  const create = !baseline;
  const canWrite = useChannelWriteGuard(create || writable);
  useDraftReloadWarning((create && edited) || pending !== undefined);
  function editDraft(next: Fields) {
    if (!canWrite() || pending || busy) return;
    setEdited(true);
    setError(undefined);
    setDraft(next);
    if (create && recoverDraft)
      writeNewChannelDraft(
        ownerSubject,
        next,
        initialDraft?.privateFields === true || Object.keys(credentials).length > 0,
      );
  }
  function editCredentials(next: Fields) {
    if (!canWrite() || pending || busy) return;
    setEdited(true);
    setError(undefined);
    setCredentials(next);
    if (create && recoverDraft) writeNewChannelDraft(ownerSubject, draft, true);
  }
  const checkingCustomSupport = create && customExecutionWaiting(discovery, draft);
  const additional = section.kind === 'consumer' && section.id !== 'default';
  const limitErrors = routingLimitErrors(routing);
  const invalidRoutingLimits =
    section.kind === 'channel' && Object.values(limitErrors).some(Boolean);
  async function send(request: Pending) {
    if (!canWrite() || busy) return;
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    try {
      const value = await serviceRequest<ChannelListItem>('event-gateway', request.path, {
        method: request.method,
        body: request.body,
        headers: { 'Idempotency-Key': request.key },
      });
      if (baseline && hasChannelAddress(value) && !channelResponseMatches(baseline, value))
        throw new Error(
          'The saved channel identity or version could not be confirmed. Keep the original request and refresh.',
        );
      setPending(undefined);
      setEdited(false);
      if (create && recoverDraft) clearNewChannelDraft(ownerSubject, request.record);
      if (hasChannelAddress(value)) {
        setBaseline(value);
        if (request.kind === 'channel') {
          setDraft((current) =>
            confirmedDraft(
              current,
              channelDraft(value),
              request.section,
              object(JSON.parse(request.body)),
            ),
          );
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
      if (
        e instanceof ServiceError &&
        ((e.status === 400 && (!create || e.code !== 'provider_rejected')) ||
          (e.status === 409 &&
            e.code === 'conflict' &&
            e.message.startsWith('channel_creation_pending:') &&
            !request.uncertain))
      ) {
        // This precondition is checked before any update effect. Keep the
        // draft editable; an uncertain write or version conflict keeps its key.
        setPending(undefined);
        if (create && recoverDraft) releaseNewChannelDraft(ownerSubject, request.record);
      } else setPending({ ...request, uncertain: true });
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
    if (!canWrite() || busy) return;
    if (pending) {
      void send(pending);
      return;
    }
    if (kind === 'routing' && invalidRoutingLimits) return;
    try {
      const record = baseline?.record_id ?? crypto.randomUUID();
      let projected = baseline ? selectedDraft(channelDraft(baseline), draft, section) : draft;
      if (kind === 'channel' && (create || section.kind === 'channel')) {
        const name = text(projected.name).trim();
        if (!name) throw new Error(`/name: ${channelNameRequired}`);
        projected = { ...projected, name };
      }
      const selectedCredentials = baseline
        ? nodeCredentials(credentials, section, projected)
        : channelCredentials(credentials, projected, true);
      if (kind === 'channel') {
        validateConnectorFields(projected);
        validateReplyFields(projected, baseline ? channelDraft(baseline) : undefined);
        validateRelayAuthoring(
          discovery,
          projected,
          baseline ? channelDraft(baseline) : undefined,
          selectedCredentials,
        );
        validateAuthoringRules(discovery, projected, baseline ? channelDraft(baseline) : undefined);
      }
      const body: Fields =
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
          : {
              ...channelRequest(projected, selectedCredentials, baseline, record),
              ...(!baseline ? customExecutionRequest(discovery, projected) : {}),
            };
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
      if (create && recoverDraft)
        writeNewChannelDraft(ownerSubject, draft, Object.keys(credentials).length > 0, record);
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
    <div className="channel-node-editor" ref={editor}>
      {create && initialDraft?.privateFields ? (
        <p role="status">
          Draft restored. Re-enter any private credentials, environment overrides or MCP server
          settings before creating.
        </p>
      ) : null}
      {!additional ? (
        <form
          className="service-form"
          onInvalidCapture={(event) => {
            const first = event.currentTarget.querySelector<HTMLElement>(
              'input:invalid, textarea:invalid',
            );
            revealInvalidField(first ?? (event.target as HTMLElement));
            first?.focus();
          }}
          onSubmit={(e) => {
            e.preventDefault();
            submit('channel');
          }}
        >
          <fieldset
            className="channel-form-section"
            disabled={!writable || pending !== undefined || busy}
          >
            <ValidationMessageScope value={!pending && !busy ? error : undefined}>
              <HmacCredentialsScope value={credentials} create={create}>
                <NodeFields
                  active={active}
                  draft={draft}
                  onChange={editDraft}
                  credentials={credentials}
                  onCredentials={editCredentials}
                  section={section}
                  create={create}
                  sourceCount={baseline?.definition.sources.length ?? 0}
                  retainedAckDelay={object(baseline?.definition.reply).ack_after_ms}
                  savedCustomExecution={baseline?.definition.durable_custom_execution}
                  denied={denied}
                />
              </HmacCredentialsScope>
            </ValidationMessageScope>
          </fieldset>
          {!pending ? (
            <Button
              type="submit"
              variant="primary"
              disabled={!writable || busy || checkingCustomSupport}
            >
              {checkingCustomSupport
                ? discovery.status === 'loading'
                  ? 'Checking support…'
                  : 'Check support to continue'
                : create
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
          <fieldset
            className="channel-form-section"
            disabled={!writable || pending !== undefined || busy}
          >
            {section.kind === 'channel' ? (
              <RoutingFields value={routing} onChange={setRouting} limitErrors={limitErrors} />
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
              disabled={
                !writable ||
                busy ||
                invalidRoutingLimits ||
                (section.kind === 'consumer' && !activeConsumer)
              }
            >
              {section.kind === 'channel' ? 'Save routing' : 'Save consumer policy'}
            </Button>
          ) : null}
        </form>
      ) : null}
      {error ? <ValidationError error={error} canEdit={!pending && !busy} root={editor} /> : null}
      {notice ? (
        <p role="status">{notice}</p>
      ) : baseline?.native_discord_activation ? (
        <p role="status">{baseline.native_discord_activation.detail}</p>
      ) : null}
      {pending ? (
        <div className="service-actions">
          <Button variant="primary" disabled={!writable || busy} onClick={() => void send(pending)}>
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
export { DeleteChannel } from './DeleteChannel';
