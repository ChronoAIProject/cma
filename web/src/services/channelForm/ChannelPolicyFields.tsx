import { useRelayCapabilities } from '../RelayCapabilities';
import { Button } from '../../ui/Button';
import { AdmissionFields } from './AdmissionFields';
import { Choice, NumberField, Section, StringList, TextField, Toggle } from './Fields';
import { object, optional, records, strings, type Fields } from './model';
const partitionKeys = [
  { value: 'conversation_and_sender', label: 'Each sender in each conversation' },
  { value: 'conversation', label: 'Each conversation' },
  { value: 'thread', label: 'Each thread' },
  { value: 'fixed', label: 'One shared conversation' },
  { value: 'event_subject', label: 'Each event subject' },
  { value: 'per_event', label: 'A new conversation for every event' },
];
export function ChannelPolicyFields({
  value,
  onChange,
  hideConnection = false,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
  hideConnection?: boolean;
}) {
  const discovery = useRelayCapabilities();
  const nativeLark = records(value.sources).some((source) => source.type === 'native_lark');
  const threadWritesDisabled = discovery.authoring?.thread_contract_writes_enabled === false;
  const partition = object(value.partition),
    reply = object(value.reply),
    sinks = records(reply.sinks),
    thread = object(reply.thread_contract);
  const set = (key: string, next: unknown) => onChange(optional(value, key, next));
  const setReply = (key: string, next: unknown) => set('reply', optional(reply, key, next));
  return (
    <>
      <Section
        title="Channel settings"
        description="Give the channel a recognizable name and manage any shared connection."
      >
        <TextField
          label="Channel name"
          value={value.name}
          onChange={(v) => set('name', v)}
          required
        />
        {!hideConnection ? (
          <>
            <TextField
              label="Connection reference"
              value={value.connection_ref}
              onChange={(v) => set('connection_ref', v || undefined)}
              hint="CMA-managed Telegram connection reference, not a NyxID route or bot ID. Leave unchanged to keep the current connection."
            />
            <Toggle
              label="Override connection admission"
              value={value.connection_admission != null}
              onChange={(enabled) =>
                set('connection_admission', enabled ? { type: 'all' } : undefined)
              }
            />
            {value.connection_admission != null ? (
              <AdmissionFields
                label="Connection admission"
                value={value.connection_admission}
                onChange={(v) => set('connection_admission', v)}
              />
            ) : null}
          </>
        ) : null}
      </Section>
      <Section
        title="Conversation grouping"
        description="Messages with the same grouping share an agent conversation and its history. Choose separate conversations when senders should not share context."
      >
        <Choice
          label="Group incoming messages by"
          value={partition.key}
          options={partitionKeys}
          onChange={(key) =>
            set('partition', { ...partition, version: partition.version ?? 1, key })
          }
        />
        <Choice
          label="Group custom events by"
          value={partition.custom_key ?? ''}
          options={[{ value: '', label: 'Use the same grouping' }, ...partitionKeys]}
          onChange={(v) => set('partition', optional(partition, 'custom_key', v || undefined))}
        />
      </Section>
      <Section
        title="Replies"
        description="Choose when answers are sent and where they go. The original conversation returns the answer to the sender."
      >
        <Toggle
          label="Send final answers automatically"
          hint="Send the agent’s completed answer without requiring an explicit reply tool call."
          value={reply.auto_final}
          onChange={(v) => setReply('auto_final', v)}
        />
        <NumberField
          label="Acknowledgement delay (ms)"
          value={reply.ack_after_ms ?? 0}
          onChange={(v) => setReply('ack_after_ms', v)}
        />
        {sinks.map((sink, index) => (
          <Section key={index} title={`Reply destination ${index + 1}`}>
            <Choice
              label={`Destination ${index + 1} type`}
              value={sink.type}
              options={[
                { value: 'source', label: 'Original conversation' },
                { value: 'result_webhook', label: 'Result webhook' },
                { value: 'sync', label: 'Synchronous response' },
              ]}
              onChange={(type) =>
                setReply(
                  'sinks',
                  sinks.map((v, i) =>
                    i === index
                      ? type === 'result_webhook'
                        ? { type, url: '', hmac_key_id: '' }
                        : { type }
                      : v,
                  ),
                )
              }
            />
            {sink.type === 'result_webhook' ? (
              <>
                <TextField
                  label={`Destination ${index + 1} URL`}
                  value={sink.url}
                  onChange={(url) =>
                    setReply(
                      'sinks',
                      sinks.map((v, i) => (i === index ? { ...v, url } : v)),
                    )
                  }
                  required
                />
                <TextField
                  label={`Destination ${index + 1} HMAC key ID`}
                  value={sink.hmac_key_id}
                  onChange={(hmac_key_id) =>
                    setReply(
                      'sinks',
                      sinks.map((v, i) => (i === index ? { ...v, hmac_key_id } : v)),
                    )
                  }
                  required
                />
              </>
            ) : null}
            <Button
              onClick={() =>
                setReply(
                  'sinks',
                  sinks.filter((_, i) => i !== index),
                )
              }
            >
              Remove destination {index + 1}
            </Button>
          </Section>
        ))}
        <Button
          disabled={sinks.length >= 3}
          onClick={() => setReply('sinks', [...sinks, { type: 'source' }])}
        >
          Add reply destination
        </Button>
      </Section>
      {!nativeLark || reply.thread_contract != null ? (
        <Section
          title="Native threads"
          description="Keep supported platform threads connected to the same agent conversation."
        >
          <Toggle
            label="Enable verified thread following"
            value={reply.thread_contract != null}
            disabled={(threadWritesDisabled || nativeLark) && reply.thread_contract == null}
            onChange={(enabled) =>
              setReply('thread_contract', enabled ? { version: 1, follow_chat_ids: [] } : undefined)
            }
          />
          <p className="channel-field-hint">
            {threadWritesDisabled
              ? 'New thread-following writes are disabled on this gateway. Existing settings are retained. '
              : null}
            Available for a single verified NyxID Lark or Feishu producer with a bot ID.
          </p>
          {reply.thread_contract != null ? (
            <StringList
              label="Follow chat IDs"
              value={strings(thread.follow_chat_ids)}
              onChange={(follow_chat_ids) =>
                setReply('thread_contract', { ...thread, follow_chat_ids })
              }
              max={256}
            />
          ) : null}
        </Section>
      ) : null}
    </>
  );
}
