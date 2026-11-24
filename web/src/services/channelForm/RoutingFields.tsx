import { Button } from '../../ui/Button';
import { Choice, NumberField, Section, StringList, TextField, Toggle } from './Fields';
import { object, optional, records, strings, without, type Fields } from './model';

const routingLimits = {
  max_targets: 8,
  max_hops: 4,
  per_consumer_per_minute: 30,
  per_channel_per_minute: 60,
} as const;

export function routingLimitErrors(value: Fields) {
  const events = object(value.agent_events);
  const invalid = (candidate: unknown, maximum: number) =>
    typeof candidate !== 'number' ||
    !Number.isInteger(candidate) ||
    candidate < 1 ||
    candidate > maximum
      ? `Enter a whole number from 1 to ${maximum}.`
      : undefined;
  return {
    max_targets: invalid(value.max_targets ?? 4, routingLimits.max_targets),
    max_hops: invalid(events.max_hops ?? 2, routingLimits.max_hops),
    per_consumer_per_minute: invalid(
      events.per_consumer_per_minute ?? 10,
      routingLimits.per_consumer_per_minute,
    ),
    per_channel_per_minute: invalid(
      events.per_channel_per_minute ?? 30,
      routingLimits.per_channel_per_minute,
    ),
  };
}

export const initialRouting = (): Fields => ({
  version: 1,
  default_consumer: { name: 'default' },
  consumers: [],
  rules: [],
  default_targets: ['default'],
  max_targets: 4,
  reply_label: 'auto',
});
export function routingRequest(value: Fields): Fields {
  return {
    ...value,
    consumers: records(value.consumers).map((v) =>
      without(v, [
        'profile',
        'profile_digest',
        'provider',
        'agent_key_hash',
        'pending',
        'created_at',
      ]),
    ),
  };
}
export function ConsumerPolicyFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
}) {
  const reply = object(value.reply),
    publish = object(value.publish);
  const set = (key: string, next: unknown) => onChange(optional(value, key, next));
  return (
    <Section
      title="Consumer policy"
      description="Control how this agent is addressed, which senders it accepts, and how it replies."
    >
      <TextField
        label="Consumer name"
        value={value.name}
        onChange={(v) => set('name', v)}
        required
      />
      <TextField
        label="Reply display name"
        value={value.label}
        onChange={(v) => set('label', v || undefined)}
      />
      <Toggle
        label="Allow mentions and direct addressing"
        value={value.mentionable ?? true}
        onChange={(v) => set('mentionable', v)}
      />
      <Choice
        label="Events from agents and services"
        value={value.non_human == null ? 'inherit' : value.non_human ? 'allow' : 'deny'}
        options={[
          { value: 'inherit', label: 'Provider default' },
          { value: 'allow', label: 'Accept' },
          { value: 'deny', label: 'Ignore' },
        ]}
        onChange={(v) => set('non_human', v === 'inherit' ? undefined : v === 'allow')}
      />
      <Choice
        label="Automatic final replies"
        value={reply.auto_final == null ? 'inherit' : reply.auto_final ? 'on' : 'off'}
        options={[
          { value: 'inherit', label: 'Channel default' },
          { value: 'on', label: 'On' },
          { value: 'off', label: 'Off' },
        ]}
        onChange={(v) =>
          set('reply', optional(reply, 'auto_final', v === 'inherit' ? undefined : v === 'on'))
        }
      />
      <Choice
        label="Consumer reply destination"
        value={reply.destination ?? 'origin'}
        options={[
          { value: 'origin', label: 'Original conversation' },
          { value: 'default', label: 'Channel default destination' },
        ]}
        onChange={(destination) => set('reply', { ...reply, destination })}
      />
      <Toggle
        label="Allow event handoffs"
        value={publish.enabled}
        onChange={(enabled) => set('publish', { ...publish, enabled })}
      />
      {publish.enabled === true ? (
        <StringList
          label="Handoff consumer IDs"
          value={strings(publish.targets)}
          onChange={(targets) => set('publish', { ...publish, targets })}
        />
      ) : null}
    </Section>
  );
}
export function RoutingFields({
  value,
  onChange,
  limitErrors,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
  limitErrors: ReturnType<typeof routingLimitErrors>;
}) {
  const addressing = object(value.addressing),
    events = object(value.agent_events),
    rules = records(value.rules),
    destination = object(value.default_reply);
  const set = (key: string, next: unknown) => onChange(optional(value, key, next));
  const updateRule = (index: number, next: Fields) =>
    set(
      'rules',
      rules.map((v, i) => (i === index ? next : v)),
    );
  return (
    <>
      <Section
        title="Event routing"
        description="Choose the default recipients and how a sender can address a particular agent."
      >
        <StringList
          label="Default consumer IDs"
          value={strings(value.default_targets)}
          onChange={(v) => set('default_targets', v)}
          max={8}
        />
        <NumberField
          label="Maximum consumers per event"
          value={value.max_targets ?? 4}
          onChange={(v) => set('max_targets', v)}
          min={1}
          max={routingLimits.max_targets}
          validationMessage={limitErrors.max_targets}
        />
        <Choice
          label="Reply labels"
          value={value.reply_label ?? 'auto'}
          options={['auto', 'always', 'never']}
          onChange={(v) => set('reply_label', v)}
        />
        {(
          [
            ['mentions', 'Route by @name mentions'],
            ['command', 'Route by /to commands'],
            ['event_field', 'Route by event target field'],
            ['unknown_target_notice', 'Explain unknown targets'],
          ] as const
        ).map(([key, label]) => (
          <Toggle
            key={key}
            label={label}
            value={addressing[key] ?? true}
            onChange={(v) => set('addressing', { ...addressing, [key]: v })}
          />
        ))}
      </Section>
      <Section title="Routing rules">
        <p className="channel-field-hint">
          Rules run in order. The first match chooses the consumers. Empty match fields allow any
          value.
        </p>
        {rules.map((rule, index) => {
          const when = object(rule.when);
          return (
            <Section key={index} title={`Rule ${index + 1}`}>
              <TextField
                label={`Rule ${index + 1} ID`}
                value={rule.id}
                onChange={(id) => updateRule(index, { ...rule, id })}
                required
              />
              {(
                [
                  ['sources', 'Source IDs'],
                  ['kinds', 'Activity kinds'],
                  ['chats', 'Chat IDs'],
                  ['senders', 'Sender IDs'],
                  ['event_types', 'Event types'],
                  ['event_sources', 'Event origins'],
                ] as const
              ).map(([key, label]) => (
                <StringList
                  key={key}
                  label={`Rule ${index + 1} ${label}`}
                  value={strings(when[key])}
                  onChange={(v) => updateRule(index, { ...rule, when: { ...when, [key]: v } })}
                />
              ))}
              <StringList
                label={`Rule ${index + 1} target IDs`}
                value={strings(rule.targets)}
                onChange={(targets) => updateRule(index, { ...rule, targets })}
                max={8}
              />
              <div className="service-actions">
                <Button
                  disabled={index === 0}
                  onClick={() => {
                    const next = [...rules];
                    [next[index - 1], next[index]] = [next[index]!, next[index - 1]!];
                    set('rules', next);
                  }}
                >
                  Move rule {index + 1} up
                </Button>
                <Button
                  onClick={() =>
                    set(
                      'rules',
                      rules.filter((_, i) => i !== index),
                    )
                  }
                >
                  Remove rule {index + 1}
                </Button>
              </div>
            </Section>
          );
        })}
        <Button
          disabled={rules.length >= 32}
          onClick={() =>
            set('rules', [
              ...rules,
              {
                id: `rule_${crypto.randomUUID().replaceAll('-', '')}`,
                when: {},
                targets: ['default'],
              },
            ])
          }
        >
          Add routing rule
        </Button>
      </Section>
      <Section
        title="Default reply destination"
        description="Used when a consumer chooses the channel default instead of replying to the original conversation."
      >
        <Choice
          label="Default reply type"
          value={destination.type ?? ''}
          options={[
            { value: '', label: 'None' },
            { value: 'telegram_chat', label: 'Telegram chat' },
            { value: 'relay_conversation', label: 'NyxID conversation' },
            { value: 'sinks', label: 'Channel result webhooks' },
          ]}
          onChange={(type) =>
            set(
              'default_reply',
              !type
                ? undefined
                : type === 'sinks'
                  ? { type }
                  : type === 'telegram_chat'
                    ? { type, source_id: 'telegram', chat_id: '' }
                    : { type, source_id: 'relay_telegram', route_id: '' },
            )
          }
        />
        {['telegram_chat', 'relay_conversation'].includes(String(destination.type)) ? (
          <TextField
            label="Reply source ID"
            value={destination.source_id}
            onChange={(source_id) => set('default_reply', { ...destination, source_id })}
            required
          />
        ) : null}
        {destination.type === 'telegram_chat' ? (
          <>
            <TextField
              label="Reply chat ID"
              value={destination.chat_id}
              onChange={(chat_id) => set('default_reply', { ...destination, chat_id })}
              required
            />
            <TextField
              label="Reply thread ID"
              value={destination.thread_id}
              onChange={(v) =>
                set('default_reply', optional(destination, 'thread_id', v || undefined))
              }
            />
          </>
        ) : null}
        {destination.type === 'relay_conversation' ? (
          <TextField
            label="Reply route ID"
            value={destination.route_id}
            onChange={(route_id) => set('default_reply', { ...destination, route_id })}
            required
          />
        ) : null}
      </Section>
      <Section
        title="Agent event limits"
        description="Bound handoffs between agents to avoid loops and excessive message traffic."
      >
        <NumberField
          label="Maximum handoff hops"
          value={events.max_hops ?? 2}
          min={1}
          max={routingLimits.max_hops}
          validationMessage={limitErrors.max_hops}
          onChange={(max_hops) =>
            set('agent_events', {
              per_consumer_per_minute: 10,
              per_channel_per_minute: 30,
              ...events,
              max_hops,
            })
          }
        />
        <NumberField
          label="Events per consumer per minute"
          value={events.per_consumer_per_minute ?? 10}
          min={1}
          max={routingLimits.per_consumer_per_minute}
          validationMessage={limitErrors.per_consumer_per_minute}
          onChange={(per_consumer_per_minute) =>
            set('agent_events', {
              max_hops: 2,
              per_channel_per_minute: 30,
              ...events,
              per_consumer_per_minute,
            })
          }
        />
        <NumberField
          label="Events per channel per minute"
          value={events.per_channel_per_minute ?? 30}
          min={1}
          max={routingLimits.per_channel_per_minute}
          validationMessage={limitErrors.per_channel_per_minute}
          onChange={(per_channel_per_minute) =>
            set('agent_events', {
              max_hops: 2,
              per_consumer_per_minute: 10,
              ...events,
              per_channel_per_minute,
            })
          }
        />
      </Section>
    </>
  );
}
