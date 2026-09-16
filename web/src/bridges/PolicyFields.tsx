import type { BridgeAdmission, BridgeConfiguration } from '@chronoai/cma-sdk';
import { useState } from 'react';
import { Picker } from '../ui/Picker';
import { Input } from '../ui/Input';

type Scoped = Extract<BridgeAdmission, { kind: 'scoped' }>['policy'];
const limitFields: readonly {
  key: keyof BridgeConfiguration['limits'];
  label: string;
  max: number;
}[] = [
  { key: 'max_partitions', label: 'Maximum conversations', max: 100000 },
  { key: 'max_sandboxes', label: 'Maximum Sandboxes', max: 100000 },
  { key: 'max_queued_messages', label: 'Maximum queued messages', max: 100000 },
  { key: 'max_messages_per_minute', label: 'Messages per minute', max: 100000 },
  { key: 'max_payload_bytes', label: 'Maximum message size in bytes', max: 1048576 },
  { key: 'inbox_retention_seconds', label: 'Message retention in seconds', max: 2592000 },
  {
    key: 'reply_secret_retention_seconds',
    label: 'Reply credential retention in seconds',
    max: 2592000,
  },
];
function scoped(value: BridgeAdmission): Scoped {
  if (value.kind === 'scoped') return value.policy;
  return {
    senders: value.kind === 'all' ? { kind: 'open' } : { kind: 'allowlist', ids: value.sender_ids },
    chats: { kind: 'open' },
    groups: 'all',
  };
}
function IdentityPolicy({
  label,
  value,
  disabled,
  onChange,
}: {
  readonly label: string;
  readonly value: Scoped['senders'];
  readonly disabled: boolean;
  readonly onChange: (value: Scoped['senders']) => void;
}) {
  const [ids, setIds] = useState(value.kind === 'allowlist' ? value.ids.join(', ') : '');
  return (
    <div>
      <label>
        {label}
        <Picker
          label={`Allowed ${label.toLowerCase()}`}
          value={value.kind}
          disabled={disabled}
          options={[
            { value: 'deny', label: 'None' },
            { value: 'allowlist', label: 'Only selected IDs' },
            { value: 'open', label: 'All' },
          ]}
          onChange={(kind) => {
            setIds('');
            onChange(kind === 'allowlist' ? { kind, ids: [] } : { kind });
          }}
        />
      </label>
      {value.kind === 'allowlist' ? (
        <label>
          {label} IDs, separated by commas
          <Input
            value={ids}
            disabled={disabled}
            onChange={(event) => {
              setIds(event.target.value);
              onChange({
                kind: 'allowlist',
                ids: [
                  ...new Set(
                    event.target.value
                      .split(',')
                      .map((id) => id.trim())
                      .filter(Boolean),
                  ),
                ],
              });
            }}
          />
        </label>
      ) : null}
    </div>
  );
}
export function PolicyFields({
  value,
  disabled,
  onChange,
}: {
  readonly value: BridgeConfiguration;
  readonly disabled: boolean;
  readonly onChange: (value: BridgeConfiguration) => void;
}) {
  const policy = scoped(value.admission);
  const update = (next: Scoped) =>
    onChange({ ...value, admission: { kind: 'scoped', policy: next } });
  return (
    <>
      <fieldset disabled={disabled} className="bridges__fields">
        <legend>Who can send messages</legend>
        <IdentityPolicy
          label="Senders"
          value={policy.senders}
          disabled={disabled}
          onChange={(senders) => update({ ...policy, senders })}
        />
        <IdentityPolicy
          label="Chats"
          value={policy.chats}
          disabled={disabled}
          onChange={(chats) => update({ ...policy, chats })}
        />
        <label>
          Group messages
          <Picker
            label="Group messages"
            value={policy.groups}
            disabled={disabled}
            options={[
              { value: 'deny', label: 'Refuse group messages' },
              { value: 'all', label: 'Allow messages from approved senders and chats' },
              {
                value: 'mention',
                label: 'Only mentions of this bot',
                unavailable: 'Selected bot identity cannot yet be verified for mentions',
              },
              {
                value: 'reply_to_bot',
                label: 'Only replies to this bot',
                unavailable: 'Selected bot identity cannot yet be verified for replies',
              },
              {
                value: 'mention_or_reply_to_bot',
                label: 'Mentions or replies to this bot',
                unavailable: 'Selected bot identity cannot yet be verified',
              },
            ]}
            onChange={(groups) => update({ ...policy, groups })}
          />
        </label>
        <label>
          Channel replies
          <Picker
            label="Channel replies"
            value={value.reply}
            disabled={disabled}
            options={[
              { value: 'final_response', label: 'Let the Agent choose when to reply' },
              {
                value: 'final_response_and_approvals',
                label: 'Agent replies and approval controls',
                unavailable: 'Verified channel approval controls are not available yet',
              },
              { value: 'disabled', label: 'Disable channel replies' },
            ]}
            onChange={(reply) => onChange({ ...value, reply })}
          />
        </label>
        <p>
          Agents created by this channel can send updates while they work. Each reply is an explicit
          Agent tool call; final answers are not sent automatically. Manually created Agents and
          Trigger sessions do not receive the reply tool.
        </p>
      </fieldset>
      <fieldset disabled={disabled} className="bridges__fields">
        <legend>Capacity and retention</legend>
        <div className="bridges__grid">
          {limitFields.map(({ key, label, max }) => (
            <label key={key}>
              {label}
              <Input
                type="number"
                min={1}
                max={max}
                step={1}
                value={Number.isFinite(value.limits[key]) ? value.limits[key] : ''}
                onChange={(event) =>
                  onChange({
                    ...value,
                    limits: { ...value.limits, [key]: event.target.valueAsNumber },
                  })
                }
              />
            </label>
          ))}
        </div>
        <p>
          Sandbox count cannot exceed conversation count. Reply credentials expire no later than
          retained messages; NyxID’s own reply deadline also applies.
        </p>
      </fieldset>
    </>
  );
}
