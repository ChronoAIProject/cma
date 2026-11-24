import { useContext, useEffect, useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import type { Channel } from './ChannelEditor';
import { OpenProducerConfiguration } from './ConfigurationSections';
import { Choice, TextField, Toggle } from './channelForm/Fields';
import { object, list, text, type ChannelSelection } from './channelTopology';
import { segment, serviceRequest, ServiceError, type Denied } from './client';
import { useRelayCapabilities } from './RelayCapabilities';
import { useChannelWriteGuard } from './useChannelWriteGuard';
import { OutboundMediaFields, OutboundMediaReview } from './OutboundMediaFields';
import {
  composeOutboundMessage,
  nativeAttachments,
  outboundAttachmentError,
  type OutboundMediaDraft,
} from './outboundMedia';

import { useOutboundJournal, validOutboundReceipt, type OutboundReceipt } from './outboundJournal';
import { outboundSeparateMessageEnabled, outboundStatus } from './outboundStatus';
import { PreviousOutboundDelivery } from './PreviousOutboundDelivery';

interface Props {
  readonly writable?: boolean;
  readonly owner: string;
  readonly channel: Channel;
  readonly selection: ChannelSelection;
  readonly denied: Denied;
  readonly onAvailability?: (available: boolean) => void;
}
/** Account/channel changes reset visible state without erasing uncertain operations. */
export function ChannelOutboundMessage(props: Props) {
  return <Composer key={JSON.stringify([props.owner, props.channel.record_id])} {...props} />;
}
function Composer({ owner, channel, selection, denied, onAvailability, writable = true }: Props) {
  const canWrite = useChannelWriteGuard(writable);
  const configureRoutes = useContext(OpenProducerConfiguration);
  const capabilities = useRelayCapabilities();
  const journal = useOutboundJournal(owner, channel.record_id);
  const { draft, receipt, uncertain = false, preparationExpired = false } = journal.intent ?? {};
  const status = receipt ? outboundStatus(receipt, uncertain, preparationExpired) : undefined;
  const [acknowledged, setAcknowledged] = useState(false);
  const [message, setMessage] = useState('');
  const [media, setMedia] = useState<OutboundMediaDraft[]>([]);
  const [route, setRoute] = useState('');
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const active = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => active.current?.abort(), []);
  const source =
    selection.kind === 'producer' ? object(channel.definition.sources[selection.index]) : {};
  const routes = list(source.route_ids).filter(
    (value): value is string => typeof value === 'string' && value.trim().length > 0,
  );
  const selectedRoute = routes.includes(route) ? route : (routes[0] ?? '');
  const base = `/v1/channels/${segment(channel.channel_id)}/outbound-messages`;
  const enabled = capabilities.authoring?.outbound_messages_writes_enabled === true;
  const relay = source.type === 'nyxid_relay';
  const canCompose = enabled && relay && routes.length > 0;
  const mediaEnabled =
    capabilities.authoring?.native_media_writes_enabled === true &&
    ['telegram', 'lark', 'feishu', 'discord', 'whatsapp'].includes(
      draft ? draft.body.source_id.replace(/^relay_/, '') : text(source.platform),
    );
  const mediaError = nativeAttachments(media)
    .map((item, index) => {
      const error = outboundAttachmentError(item);
      return error ? `Attachment ${index + 1}: ${error}` : '';
    })
    .find(Boolean);
  const hasMedia = (draft?.body.message.native_attachments?.length ?? media.length) > 0;
  const mediaUnavailable = hasMedia && !mediaEnabled;
  const canReview =
    canCompose && (!!message.trim() || media.length > 0) && !mediaError && !mediaUnavailable;

  async function act(action: 'prepare' | 'send' | 'read' | 'cancel') {
    if ((action === 'prepare' || action === 'send') && !canWrite()) return;
    if (active.current || (journal.error && action !== 'read')) return;
    if (
      action === 'send' &&
      (!enabled ||
        mediaUnavailable ||
        !receipt ||
        !outboundStatus(receipt, uncertain, preparationExpired).send)
    )
      return;
    if (action === 'cancel' && !status?.cancel) return;
    // A retained preparation recovers its exact existing operation. New writes
    // still require current capabilities; a gate change cannot hide its status.
    if (action === 'prepare' && !draft && !canReview) return;
    const captured = draft ?? {
      key: crypto.randomUUID(),
      body: {
        source_id: `relay_${text(source.platform)}`,
        route_id: selectedRoute,
        message: composeOutboundMessage(message, media),
      },
    };
    // Save the exact request and uncertainty before any network effect. A reload
    // can check the same operation; it never invents a replacement key or sends.
    if (
      action !== 'read' &&
      !journal.retain({ ...journal.intent, draft: captured, receipt, uncertain: true })
    )
      return;
    const abort = new AbortController();
    active.current = abort;
    setPending(true);
    setError('');
    const timeout =
      action === 'send' && captured.body.message.native_attachments?.length ? 65000 : 15000;
    const signal = AbortSignal.any([abort.signal, AbortSignal.timeout(timeout)]);
    try {
      let value: OutboundReceipt;
      if (action === 'prepare') {
        value = await serviceRequest('event-gateway', base, {
          method: 'POST',
          signal,
          headers: { 'Idempotency-Key': captured.key },
          body: JSON.stringify(captured.body),
        });
      } else {
        if (!receipt) return;
        value = await serviceRequest(
          'event-gateway',
          `${base}/${segment(receipt.delivery_id)}${action === 'send' ? '/send' : ''}`,
          {
            method: action === 'read' ? 'GET' : action === 'cancel' ? 'DELETE' : 'POST',
            signal,
            ...(action === 'send' ? { body: JSON.stringify(captured.body.message) } : {}),
          },
        );
      }
      if (abort.signal.aborted) return;
      if (
        !validOutboundReceipt(value) ||
        value.route_id !== captured.body.route_id ||
        (receipt && value.delivery_id !== receipt.delivery_id)
      )
        throw new Error(
          'The message status could not be verified. Your original request is saved.',
        );
      journal.retain({
        ...journal.intent,
        draft: captured,
        receipt: value,
        uncertain: value.delivery?.outcome === 'unknown',
      });
    } catch (cause) {
      if (abort.signal.aborted) return;
      if (action === 'read')
        journal.retain({ ...journal.intent, draft: captured, receipt, uncertain: true });
      if (cause instanceof ServiceError && cause.status === 401) denied(cause.status);
      if (
        action === 'send' &&
        cause instanceof ServiceError &&
        cause.status === 410 &&
        cause.message === 'outbound_preparation_expired'
      ) {
        journal.retain({ draft: captured, receipt, uncertain: true, preparationExpired: true });
        return;
      }
      // Only a first, explicitly invalid preparation can be edited immediately.
      // An earlier uncertain request retains its original key even after a 4xx.
      if (
        action === 'prepare' &&
        !draft &&
        cause instanceof ServiceError &&
        [400, 413, 422].includes(cause.status)
      )
        journal.retain();
      setError(
        action === 'send'
          ? 'Sending was not confirmed. Check the saved delivery status; do not send a second copy.'
          : cause instanceof Error
            ? cause.message
            : 'The request could not be confirmed.',
      );
    } finally {
      active.current = undefined;
      if (!abort.signal.aborted) setPending(false);
    }
  }
  const available = Boolean(
    receipt || draft || journal.previous || journal.recoveryError || (enabled && relay),
  );
  useEffect(() => {
    onAvailability?.(available);
  }, [available, onAvailability]);
  if (!available) return null;
  function resetComposer() {
    setMessage('');
    setMedia([]);
    setError('');
    setAcknowledged(false);
  }
  return (
    <section className="channel-inspector__test" aria-label="Send to saved destination">
      <h3>Send a message</h3>
      <p>
        Send through a saved NyxID conversation that permits initiated messages. Review the message
        before sending it.
      </p>
      {!receipt && !draft && enabled && relay && !routes.length ? (
        <div>
          <p>No saved destinations. Add a NyxID route ID to this producer and save it.</p>
          {configureRoutes ? <Button onClick={configureRoutes}>Configure routes</Button> : null}
        </div>
      ) : null}
      {!receipt && !draft && canCompose ? (
        <fieldset
          disabled={!writable || pending || !!journal.error}
          className="channel-form-section"
        >
          <Choice
            label="Saved destination"
            value={selectedRoute}
            options={routes}
            onChange={setRoute}
          />
          <TextField label="Message text" value={message} onChange={setMessage} multiline />
          {mediaEnabled ? <OutboundMediaFields items={media} onChange={setMedia} /> : null}
          {!mediaEnabled && media.length ? (
            <OutboundMediaReview items={nativeAttachments(media)} />
          ) : null}
          {mediaError && mediaEnabled ? (
            <p className="channel-field-hint" role="status">
              {mediaError}
            </p>
          ) : null}
        </fieldset>
      ) : null}
      {receipt || draft ? (
        <div>
          <p className="channel-field-hint">
            Destination: {draft?.body.route_id ?? receipt?.route_id}
          </p>
          <p>{draft?.body.message.text}</p>
          <OutboundMediaReview items={draft?.body.message.native_attachments ?? []} />
        </div>
      ) : null}
      {mediaUnavailable ? (
        <p className="channel-field-hint" role="status">
          Your attachments are retained. Media sending is currently unavailable on this gateway.
        </p>
      ) : null}
      <Presence>
        {journal.error || error ? <Alert tone="warning">{journal.error ?? error}</Alert> : null}
      </Presence>
      {status ? <p role="status">{status.message}</p> : null}
      {status?.separateMessage && enabled && outboundSeparateMessageEnabled() ? (
        <Toggle
          label="I understand that the previous message may have been sent and another message could duplicate it."
          value={acknowledged}
          onChange={setAcknowledged}
          disabled={pending}
        />
      ) : null}
      <div className="channel-form-actions">
        {!receipt && (draft || canCompose) ? (
          <Button
            disabled={!writable || pending || !!journal.error || (!draft && !canReview)}
            onClick={() => void act('prepare')}
          >
            {draft ? 'Check saved request' : 'Review message'}
          </Button>
        ) : null}
        {status?.send ? (
          <Button
            disabled={!writable || pending || !!journal.error || !enabled || mediaUnavailable}
            onClick={() => void act('send')}
          >
            Send message
          </Button>
        ) : null}
        {status?.cancel ? (
          <Button disabled={pending || !!journal.error} onClick={() => void act('cancel')}>
            Cancel
          </Button>
        ) : null}
        {status?.check ? (
          <Button disabled={pending} onClick={() => void act('read')}>
            Check delivery status
          </Button>
        ) : null}
        {status?.separateMessage && enabled && outboundSeparateMessageEnabled() ? (
          <Button
            disabled={pending || !!journal.error || !acknowledged}
            onClick={() => {
              if (acknowledged && journal.archive()) resetComposer();
            }}
          >
            Start a separate message
          </Button>
        ) : null}
        {status?.newMessage ? (
          <Button
            disabled={!!journal.error}
            onClick={() => {
              if (journal.retain()) resetComposer();
            }}
          >
            New message
          </Button>
        ) : null}
      </div>
      {receipt ? (
        <p className="channel-field-hint">Delivery reference: {receipt.delivery_id}</p>
      ) : null}
      {journal.recoveryError ? <Alert tone="warning">{journal.recoveryError}</Alert> : null}
      {journal.previous ? (
        <PreviousOutboundDelivery
          base={base}
          intent={journal.previous}
          retain={journal.retainPrevious}
          denied={denied}
        />
      ) : null}
    </section>
  );
}
