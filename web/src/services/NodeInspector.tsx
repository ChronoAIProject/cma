import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import {
  ConfigurationSections,
  OpenProducerConfiguration,
  scrollToConfiguration,
  useConfigurationSections,
} from './ConfigurationSections';
import './NodeInspector.css';
import { ChannelToolsets } from './ChannelToolsets';
import type { Channel } from './ChannelEditor';
import { ChannelEndpoints } from './ChannelEndpoints';
import { Modal } from '../ui/Modal';
import {
  admissionSummary,
  channelNodes,
  consumerConfig,
  list,
  object,
  platformName,
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
        : source.type === 'native_lark'
          ? { native_lark_webhook_url: channel.endpoints.native_lark_webhook_url }
          : source.type === 'whatsapp'
            ? { whatsapp_webhook_url: channel.endpoints.whatsapp_webhook_url }
            : { nyxid_callback_url: channel.endpoints.nyxid_callback_url };
  const admission = object(source.admission);
  const facts: Fact[] = [
    [
      'Access policy',
      source.type === 'custom' ? words(object(source.auth).type) : admissionSummary(admission),
    ],
  ];
  if (source.type === 'discord') {
    facts.push(['Connection', 'Direct Discord bot']);
    if (channel.native_discord_activation)
      facts.push(['Activation', channel.native_discord_activation.detail]);
  }
  if (source.type === 'native_lark') {
    const identity = object(source.identity);
    facts.push(
      ['Connection', `Direct ${platformName(identity.region)} app`],
      ['Region', platformName(identity.region)],
      ['App ID', text(identity.app_id)],
      ['Tenant key', text(identity.tenant_key)],
      ['Bot open ID', text(identity.bot_open_id)],
      ['Setup', 'Separate app and webhook; keep existing NyxID app webhooks unchanged.'],
      ['Replies', 'Text replies to admitted conversations'],
    );
  }
  if (source.type === 'whatsapp')
    facts.push(
      ['Connection', 'Direct WhatsApp Business'],
      ['Meta app ID', text(source.app_id)],
      ['Business account ID', text(source.waba_id)],
      ['Phone number ID', text(source.phone_number_id)],
    );
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
      ['Group rule', words(admission.groups)],
    );
  return (
    <>
      <Facts facts={facts} />
      {source.type !== 'discord' ? <ChannelEndpoints endpoints={endpoints} /> : null}
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
  sendAvailable = true,
  testsAvailable = true,
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
  readonly sendAvailable?: boolean;
  readonly testsAvailable?: boolean;
  readonly send?: ReactNode;
  readonly tests?: ReactNode;
  readonly activity?: ReactNode;
  readonly danger?: ReactNode;
}) {
  const heading = useId();
  const panel = useRef<HTMLElement>(null);
  const { sections: settings, register } = useConfigurationSections();
  const [activeSection, setActiveSection] = useState(`${id}-overview`);
  const requestedSection = useRef<string | undefined>(`${id}-overview`);
  const selectionId = selectionKey(selection);
  useEffect(() => {
    if (!open) return;
    requestedSection.current = `${id}-${initialSection}`;
    const frame = requestAnimationFrame(() => {
      scrollToConfiguration(`${id}-${initialSection}`, false, false);
      setActiveSection(`${id}-${initialSection}`);
    });
    return () => cancelAnimationFrame(frame);
  }, [id, open, initialSection, selectionId]);
  const { producers, consumers, toolsets } = channelNodes(channel);
  const selected = [...producers, ...consumers, toolsets].find(
    (node) => node.key === selectionKey(selection),
  );
  const newProducer =
    selection.kind === 'producer' && selection.index === channel.definition.sources.length;
  const title =
    selection.kind === 'channel'
      ? 'Channel settings'
      : selection.kind === 'toolsets'
        ? 'Event tools'
        : newProducer
          ? 'New producer'
          : (selected?.title ?? 'Node unavailable');
  function navigate(section: string) {
    requestedSection.current = section;
    setActiveSection(section);
    scrollToConfiguration(section);
  }
  const sections = [
    ['overview', 'Overview', true],
    ['configuration', 'Configuration', children],
    [
      'catalog',
      'Tool catalog',
      selection.kind === 'toolsets' ? <ChannelToolsets channel={channel} /> : null,
    ],
    ['send', 'Send message', send],
    ['tests', 'Tests', tests],
    ['activity', 'Activity', activity],
    ['delete', 'Delete', danger],
  ] as const;
  const sectionVisible = (key: string) =>
    (selection.kind !== 'toolsets' || ['overview', 'catalog'].includes(key)) &&
    (key !== 'send' || sendAvailable) &&
    (key !== 'tests' || testsAvailable);
  const navigation = sections.flatMap(([key, label, content]) => {
    if (!content || !sectionVisible(key)) return [];
    if ((key === 'configuration' || key === 'catalog') && settings.length > 0)
      return settings.map((section) => ({ id: section.id, label: section.title }));
    return [{ id: `${id}-${key}`, label }];
  });
  const navigationIds = navigation.map((entry) => entry.id).join(',');
  useEffect(() => {
    const root = panel.current;
    if (!open || !root) return;
    let frame = 0;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const top = root.getBoundingClientRect().top + 32;
        const targets = navigationIds
          .split(',')
          .map((key) => document.getElementById(key))
          .filter((el) => el !== null);
        const requested = targets.find((el) => el.id === requestedSection.current);
        const maximum = root.scrollHeight - root.clientHeight;
        const atBottom = maximum > 1 && root.scrollTop >= maximum - 1;
        const current =
          requested ??
          (atBottom
            ? targets.at(-1)
            : targets.filter((el) => el.getBoundingClientRect().top <= top).at(-1)) ??
          targets[0];
        if (current) setActiveSection(current.id);
      });
    };
    // Keep an explicit destination highlighted through smooth scrolling and
    // bottom clamping. Actual user scrolling resumes position-based tracking.
    const releaseSelection = () => {
      requestedSection.current = undefined;
    };
    const keyboardScroll = (event: KeyboardEvent) => {
      if (event.key === 'Tab') return releaseSelection();
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        (target.closest('input, textarea, select') || target.isContentEditable)
      )
        return;
      if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(event.key))
        releaseSelection();
    };
    root.addEventListener('scroll', update, { passive: true });
    root.addEventListener('wheel', releaseSelection, { passive: true });
    root.addEventListener('pointerdown', releaseSelection, { passive: true });
    root.addEventListener('keydown', keyboardScroll);
    update();
    return () => {
      root.removeEventListener('scroll', update);
      root.removeEventListener('wheel', releaseSelection);
      root.removeEventListener('pointerdown', releaseSelection);
      root.removeEventListener('keydown', keyboardScroll);
      cancelAnimationFrame(frame);
    };
  }, [open, navigationIds]);
  return (
    <Modal open={open} onClose={onClose} title={title} layout="sections" keepMounted>
      <div className="channel-config-layout">
        <nav className="channel-config-nav" aria-label="Configuration sections">
          <span className="channel-config-nav__label">
            {selection.kind === 'toolsets' ? 'Tools' : 'Settings'}
          </span>
          {navigation.map((entry) => (
            <button
              type="button"
              key={entry.id}
              aria-current={activeSection === entry.id ? 'location' : undefined}
              onClick={() => navigate(entry.id)}
            >
              {entry.label}
            </button>
          ))}
        </nav>
        <section className="channel-inspector" id={id} ref={panel} aria-labelledby={heading}>
          <section id={`${id}-overview`} tabIndex={-1} className="channel-inspector__section">
            <div className="channel-inspector__heading">
              <span>
                {selection.kind === 'channel'
                  ? 'CHANNEL'
                  : selection.kind === 'toolsets'
                    ? 'TOOLSETS'
                    : selection.kind === 'producer'
                      ? 'PRODUCER'
                      : 'CONSUMER'}
              </span>
              <h2 id={heading}>{title}</h2>
              <p>
                {selection.kind === 'channel'
                  ? 'Shared routing, conversation and reply settings.'
                  : selection.kind === 'toolsets'
                    ? 'Tools for reading and responding to channel events.'
                    : selection.kind === 'producer'
                      ? 'How this source sends events into the channel.'
                      : 'The agent that receives and responds to events.'}
              </p>
            </div>
            <div className="channel-inspector__summary" key={selectionKey(selection)}>
              {selection.kind === 'toolsets' ? null : newProducer ? (
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
                hidden={!sectionVisible(key)}
              >
                {key !== 'catalog' ? <h3>{label}</h3> : null}
                <ConfigurationSections
                  value={
                    (key === 'configuration' && selection.kind !== 'toolsets') || key === 'catalog'
                      ? register
                      : undefined
                  }
                >
                  {key === 'send' ? (
                    <OpenProducerConfiguration
                      value={() => navigate(settings[0]?.id ?? `${id}-configuration`)}
                    >
                      {content}
                    </OpenProducerConfiguration>
                  ) : (
                    content
                  )}
                </ConfigurationSections>
              </section>
            ) : null,
          )}
        </section>
      </div>
    </Modal>
  );
}
