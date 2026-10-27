import { useEffect, useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import type { Channel } from './ChannelEditor';
import { Choice, TextField } from './channelForm/Fields';
import { object, list, text, type ChannelSelection } from './channelTopology';
import { segment, serviceRequest, ServiceError, type Denied } from './client';
import { useRelayCapabilities } from './RelayCapabilities';
import { OutboundMediaFields, OutboundMediaReview } from './OutboundMediaFields';
import {
  composeOutboundMessage,
  nativeAttachments,
  outboundAttachmentError,
  type OutboundMediaDraft,
} from './outboundMedia';

import { useOutboundJournal, validOutboundReceipt, type OutboundReceipt } from './outboundJournal';

interface Props {
  readonly owner: string;
  readonly channel: Channel;
  readonly selection: ChannelSelection;
  readonly denied: Denied;
}
/** Account/channel changes reset visible state without erasing uncertain operations. */
export function ChannelOutboundMessage(props: Props) {
  return <Composer key={JSON.stringify([props.owner, props.channel.record_id])} {...props} />;
}
function Composer({ owner, channel, selection, denied }: Props) {
  const capabilities = useRelayCapabilities();
  const journal = useOutboundJournal(owner, channel.record_id);
  const { draft, receipt, uncertain = false } = journal.intent ?? {};
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
    (value): value is string => typeof value === 'string',
  );
  const selectedRoute = routes.includes(route) ? route : (routes[0] ?? '');
  const base = `/v1/channels/${segment(channel.channel_id)}/outbound-messages`;
  const enabled = capabilities.authoring?.outbound_messages_writes_enabled === true;
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
    !!selectedRoute && (!!message.trim() || media.length > 0) && !mediaError && !mediaUnavailable;

  async function act(action: 'prepare' | 'send' | 'read' | 'cancel') {
    if (active.current || journal.error) return;
    if (action === 'send' && (!enabled || mediaUnavailable)) return;
    // A retained preparation recovers its exact existing operation. New writes
    // still require current capabilities; a gate change cannot hide its status.
    if (action === 'prepare' && !draft && (!enabled || !canReview)) return;
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
    if (!journal.retain({ draft: captured, receipt, uncertain: true })) return;
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
        draft: captured,
        receipt: value,
        uncertain: value.delivery?.outcome === 'unknown',
      });
    } catch (cause) {
      if (abort.signal.aborted) return;
      if (cause instanceof ServiceError && cause.status === 401) denied(cause.status);
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
  if ((!enabled && !receipt && !draft) || (source.type !== 'nyxid_relay' && !receipt && !draft))
    return null;
  const finished =
    receipt?.status === 'cancelled' ||
    ['confirmed', 'rejected'].includes(receipt?.delivery?.outcome ?? '');
  return (
    <section className="channel-inspector__test" aria-label="Send to saved destination">
      <h3>Send a message</h3>
      <p>
        Send through a saved NyxID conversation that permits initiated messages. Review the message
        before sending it.
      </p>
      {!receipt && !draft ? (
        <fieldset disabled={pending || !!journal.error} className="channel-form-section">
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
      ) : (
        <div>
          <p className="channel-field-hint">
            Destination: {draft?.body.route_id ?? receipt?.route_id}
          </p>
          <p>{draft?.body.message.text}</p>
          <OutboundMediaReview items={draft?.body.message.native_attachments ?? []} />
        </div>
      )}
      {mediaUnavailable ? (
        <p className="channel-field-hint" role="status">
          Your attachments are retained. Media sending is currently unavailable on this gateway.
        </p>
      ) : null}
      <Presence>
        {journal.error || error ? <Alert tone="warning">{journal.error ?? error}</Alert> : null}
      </Presence>
      {receipt ? (
        <p role="status">
          {receipt.status === 'cancelled'
            ? 'Cancelled before sending.'
            : receipt.delivery?.outcome === 'confirmed'
              ? 'The platform accepted the message.'
              : receipt.delivery?.outcome === 'rejected'
                ? 'The message was not sent. Review the destination settings before trying a new message.'
                : uncertain
                  ? 'Delivery is in progress or uncertain. Check its status before continuing.'
                  : receipt.status === 'prepared'
                    ? 'Review the destination, then send when ready.'
                    : 'Delivery is in progress or uncertain. CMA will not resend it automatically.'}
        </p>
      ) : null}
      <div className="channel-form-actions">
        {!receipt ? (
          <Button
            disabled={pending || !!journal.error || (!draft && (!enabled || !canReview))}
            onClick={() => void act('prepare')}
          >
            {draft ? 'Check saved request' : 'Review message'}
          </Button>
        ) : null}
        {receipt?.status === 'prepared' && !uncertain ? (
          <Button
            disabled={pending || !!journal.error || !enabled || mediaUnavailable}
            onClick={() => void act('send')}
          >
            Send message
          </Button>
        ) : null}
        {receipt?.status === 'prepared' && !uncertain ? (
          <Button disabled={pending || !!journal.error} onClick={() => void act('cancel')}>
            Cancel
          </Button>
        ) : null}
        {receipt && !finished ? (
          <Button disabled={pending || !!journal.error} onClick={() => void act('read')}>
            Check delivery status
          </Button>
        ) : null}
        {finished ? (
          <Button
            disabled={!!journal.error}
            onClick={() => {
              if (journal.retain()) {
                setMessage('');
                setMedia([]);
                setError('');
              }
            }}
          >
            New message
          </Button>
        ) : null}
      </div>
      {receipt ? (
        <p className="channel-field-hint">Delivery reference: {receipt.delivery_id}</p>
      ) : null}
    </section>
  );
}
