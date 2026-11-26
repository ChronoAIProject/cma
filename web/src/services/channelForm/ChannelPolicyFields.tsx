import {
  ackDelayHint,
  ackDelayMessage,
  maxAckDelay,
  supportsVerifiedThreads,
} from './replyValidation';
import { useRelayCapabilities } from '../RelayCapabilities';
import { Button } from '../../ui/Button';
import { AdmissionFields } from './AdmissionFields';
import { Choice, NumberField, Section, StringList, TextField, Toggle } from './Fields';
import {
  customGroupingApplies,
  nextSink,
  partitionKeys,
  partitionKeyUnavailable,
  sinkTypeUsedBy,
} from './authoringRules';
import { object, optional, records, strings, text, type Fields } from './model';
const customPartitionKeys = partitionKeys.filter((option) =>
  ['fixed', 'event_subject', 'per_event'].includes(option.value),
);
export const channelNameRequired = 'Enter a channel name.';

export function ChannelPolicyFields({
  value,
  onChange,
  hideConnection = false,
  part = 'all',
  retainedAckDelay,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
  hideConnection?: boolean;
  part?: 'all' | 'identity' | 'policy';
  retainedAckDelay?: unknown;
}) {
  const discovery = useRelayCapabilities();
  const name = text(value.name);
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
      {part !== 'policy' ? (
        <Section
          title="Channel settings"
          description="Give the channel a recognizable name and review its shared connection."
        >
          <TextField
            path="/name"
            label="Channel name"
            value={name}
            onChange={(v) => set('name', v)}
            validationMessage={name && !name.trim() ? channelNameRequired : undefined}
            required
          />
          {!hideConnection ? (
            <>
              <TextField
                label="Connection reference"
                value={text(value.connection_ref) || 'No shared connection'}
                readOnly
                hint="This channel's saved CMA-managed connection. When empty, the channel uses its producer settings. The reference cannot be added, removed, or replaced after channel creation. To use a different managed connection, create a new channel."
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
                  path="/connection_admission"
                  label="Connection admission"
                  value={value.connection_admission}
                  onChange={(v) => set('connection_admission', v)}
                />
              ) : null}
            </>
          ) : null}
        </Section>
      ) : null}
      {part !== 'identity' ? (
        <>
          <Section
            title="Conversation grouping"
            description="Messages with the same grouping share an agent conversation and its history. Choose separate conversations when senders should not share context."
          >
            <Choice
              path="/partition/key"
              label="Group incoming messages by"
              value={partition.key}
              options={partitionKeys.map((option) => ({
                ...option,
                unavailable: partitionKeyUnavailable(discovery, option.value, value.sources),
              }))}
              onChange={(key) =>
                set('partition', {
                  // A separate custom-event grouping applies only beneath a chat grouping.
                  ...(['conversation_and_sender', 'conversation', 'thread'].includes(key)
                    ? partition
                    : optional(partition, 'custom_key', undefined)),
                  version: partition.version ?? 1,
                  key,
                })
              }
            />
            {customGroupingApplies(discovery, value.sources) || partition.custom_key != null ? (
              <Choice
                path="/partition/custom_key"
                label="Group custom events by"
                value={partition.custom_key ?? ''}
                options={[
                  {
                    value: '',
                    label: 'Use the same grouping',
                    unavailable: partitionKeyUnavailable(
                      discovery,
                      text(partition.key),
                      records(value.sources).filter((source) => source.type === 'custom'),
                    ),
                  },
                  ...customPartitionKeys,
                ]}
                onChange={(v) =>
                  set('partition', optional(partition, 'custom_key', v || undefined))
                }
              />
            ) : null}
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
              path="/reply/ack_after_ms"
              label="Acknowledgement delay (ms)"
              hint={ackDelayHint}
              max={
                reply.ack_after_ms === retainedAckDelay && typeof retainedAckDelay === 'number'
                  ? Math.max(maxAckDelay, retainedAckDelay)
                  : maxAckDelay
              }
              validationMessage={ackDelayMessage(reply.ack_after_ms ?? 0, retainedAckDelay)}
              value={reply.ack_after_ms ?? 0}
              onChange={(v) => setReply('ack_after_ms', v)}
            />
            {sinks.map((sink, index) => (
              <Section key={index} title={`Reply destination ${index + 1}`}>
                <Choice
                  path={`/reply/sinks/${index}`}
                  label={`Destination ${index + 1} type`}
                  value={sink.type}
                  options={[
                    { value: 'source', label: 'Original conversation' },
                    { value: 'result_webhook', label: 'Result webhook' },
                    { value: 'sync', label: 'Synchronous response' },
                  ].map((option) => {
                    const used = sinkTypeUsedBy(sinks, index, option.value);
                    return used
                      ? { ...option, unavailable: `Already used by destination ${used}.` }
                      : option;
                  })}
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
              onClick={() => setReply('sinks', [...sinks, nextSink(sinks)])}
            >
              Add reply destination
            </Button>
          </Section>
          {!nativeLark || reply.thread_contract != null ? (
            <Section
              path="/reply/thread_contract"
              title="Native threads"
              description="Keep supported platform threads connected to the same agent conversation."
            >
              <Toggle
                label="Enable verified thread following"
                value={reply.thread_contract != null}
                disabled={
                  (threadWritesDisabled || !supportsVerifiedThreads(value)) &&
                  reply.thread_contract == null
                }
                onChange={(enabled) =>
                  setReply(
                    'thread_contract',
                    enabled ? { version: 1, follow_chat_ids: [] } : undefined,
                  )
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
      ) : null}
    </>
  );
}
