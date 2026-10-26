import { useEffect, useId, type ReactNode } from 'react';
import type { Channel } from './ChannelEditor';
import { ChannelEndpoints } from './ChannelEndpoints';
import { Modal } from '../ui/Modal';
import {
  admissionSummary,
  channelNodes,
  consumerConfig,
  list,
  object,
  routing,
  selectionKey,
  text,
  words,
  type ChannelSelection,
} from './channelTopology';

type Fact = readonly [string, ReactNode];
function Facts({ facts }: { readonly facts: readonly Fact[] }) {
  return (
    <dl className="channel-inspector__facts">
      {facts.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}
function readableList(value: unknown, fallback = 'None'): string {
  const values = list(value).filter((entry): entry is string => typeof entry === 'string');
  return values.length > 0 ? values.join(', ') : fallback;
}
function ProducerFacts({ channel, index }: { readonly channel: Channel; readonly index: number }) {
  const source = object(channel.definition.sources[index]);
  const endpoints: Partial<Channel['endpoints']> =
    source.type === 'custom'
      ? { custom_event_url: channel.endpoints.custom_event_url }
      : source.type === 'telegram'
        ? { telegram_webhook_url: channel.endpoints.telegram_webhook_url }
        : { nyxid_callback_url: channel.endpoints.nyxid_callback_url };
  const admission = object(source.admission);
  const facts: Fact[] = [
    [
      'Access policy',
      source.type === 'custom' ? words(object(source.auth).type) : admissionSummary(admission),
    ],
  ];
  if (source.type === 'nyxid_relay')
    facts.push(
      ['Routes', readableList(source.route_ids, 'No routes attached')],
      [
        'Verification',
        source.trust_normalized === true
          ? 'NyxID normalized events'
          : 'Signed relay; platform policy applies',
      ],
    );
  if (source.bot_username) facts.push(['Bot username', text(source.bot_username)]);
  if (source.bot_id) facts.push(['Bot identity', text(source.bot_id)]);
  if (admission.type === 'allowlist')
    facts.push(['Allowed senders', readableList(admission.sender_ids)]);
  if (admission.type === 'scoped')
    facts.push(
      ['Sender rule', words(object(admission.senders).type)],
      ['Chat rule', words(object(admission.chats).type)],
      ['Group rule', words(object(admission.groups).type)],
    );
  return (
    <>
      <Facts facts={facts} />
      <ChannelEndpoints endpoints={endpoints} />
    </>
  );
}
function ConsumerFacts({ channel, id }: { readonly channel: Channel; readonly id: string }) {
  const consumer = consumerConfig(channel, id);
  const provider = object(consumer.provider);
  const target = object(provider.target);
  const reply = object(consumer.reply);
  const autoFinal =
    typeof reply.auto_final === 'boolean'
      ? reply.auto_final
      : object(channel.definition.reply).auto_final;
  return (
    <Facts
      facts={[
        ['Consumer ID', id],
        [
          'Provider',
          text(provider.slug, id === 'default' ? channel.provider.slug : 'Not recorded'),
        ],
        ['Binding', text(provider.binding_id, 'Not recorded')],
        ['Target', target.type === 'existing_agent' ? text(target.agent_id) : 'Agent from profile'],
        ['Setup', consumer.pending === true ? 'Binding pending' : 'Recorded binding'],
        [
          'Automatic reply',
          typeof autoFinal === 'boolean' ? (autoFinal ? 'Enabled' : 'Disabled') : 'Not recorded',
        ],
        ['Reply destination', reply.destination === 'default' ? 'Channel default' : 'Event origin'],
      ]}
    />
  );
}
export function NodeInspector({
  channel,
  selection,
  id,
  children,
  open = true,
  onClose = () => {},
  initialSection = 'overview',
  send,
  tests,
  activity,
  danger,
}: {
  readonly channel: Channel;
  readonly selection: ChannelSelection;
  readonly id: string;
  readonly children: ReactNode;
  readonly open?: boolean;
  readonly onClose?: () => void;
  readonly initialSection?: 'overview' | 'activity';
  readonly send?: ReactNode;
  readonly tests?: ReactNode;
  readonly activity?: ReactNode;
  readonly danger?: ReactNode;
}) {
  const heading = useId();
  const selectionId = selectionKey(selection);
  useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => jumpToSection(id, initialSection, false, false));
    return () => cancelAnimationFrame(frame);
  }, [id, open, initialSection, selectionId]);
  const { producers, consumers } = channelNodes(channel);
  const selected = [...producers, ...consumers].find(
    (node) => node.key === selectionKey(selection),
  );
  const newProducer =
    selection.kind === 'producer' && selection.index === channel.definition.sources.length;
  const title =
    selection.kind === 'channel'
      ? 'Channel settings'
      : newProducer
        ? 'New producer'
        : (selected?.title ?? 'Node unavailable');
  const sections = [
    ['overview', 'Overview', true],
    ['configuration', 'Configuration', children],
    ['send', 'Send message', send],
    ['tests', 'Tests', tests],
    ['activity', 'Activity', activity],
    ['delete', 'Delete', danger],
  ] as const;
  return (
    <Modal open={open} onClose={onClose} title={title} layout="sections" keepMounted>
      <div className="channel-config-layout">
        <nav className="channel-config-nav" aria-label="Configuration sections">
          {sections
            .filter(([, , content]) => Boolean(content))
            .map(([key, label]) => (
              <button type="button" key={key} onClick={() => jumpToSection(id, key, true)}>
                {label}
              </button>
            ))}
        </nav>
        <section className="channel-inspector" id={id} aria-labelledby={heading}>
          <section id={`${id}-overview`} tabIndex={-1} className="channel-inspector__section">
            <div className="channel-inspector__heading">
              <span>
                {selection.kind === 'channel'
                  ? 'CHANNEL'
                  : selection.kind === 'producer'
                    ? 'PRODUCER'
                    : 'CONSUMER'}
              </span>
              <h2 id={heading}>{title}</h2>
              <p>
                {selection.kind === 'channel'
                  ? 'Shared routing, conversation and reply settings.'
                  : selection.kind === 'producer'
                    ? 'How this source sends events into the channel.'
                    : 'The agent that receives and responds to events.'}
              </p>
            </div>
            <div className="channel-inspector__summary" key={selectionKey(selection)}>
              {newProducer ? (
                <p className="channel-field-hint">
                  Configure and save this producer before checking its delivery settings.
                </p>
              ) : selection.kind === 'producer' ? (
                <ProducerFacts channel={channel} index={selection.index} />
              ) : selection.kind === 'consumer' ? (
                <ConsumerFacts channel={channel} id={selection.id} />
              ) : (
                <>
                  <Facts
                    facts={[
                      ['Conversation grouping', words(object(channel.definition.partition).key)],
                      [
                        'Default recipients',
                        readableList(routing(channel).default_targets, 'Default consumer'),
                      ],
                      ['Routing rules', String(list(routing(channel).rules).length)],
                      ['Configuration version', String(channel.version)],
                    ]}
                  />
                  <ChannelEndpoints endpoints={channel.endpoints} />
                </>
              )}
            </div>
          </section>
          {sections.slice(1).map(([key, label, content]) =>
            content ? (
              <section
                key={key}
                id={`${id}-${key}`}
                tabIndex={-1}
                className="channel-inspector__section"
                aria-label={label}
                data-section={key}
              >
                <h3>{label}</h3>
                {content}
              </section>
            ) : null,
          )}
        </section>
      </div>
    </Modal>
  );
}

function jumpToSection(id: string, key: string, animate: boolean, focus = true) {
  const target = document.getElementById(`${id}-${key}`);
  target?.scrollIntoView?.({
    block: 'start',
    behavior:
      animate && !window.matchMedia('(prefers-reduced-motion: reduce)').matches
        ? 'smooth'
        : 'instant',
  });
  if (focus) target?.focus({ preventScroll: true });
}
