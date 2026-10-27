import { useRelayCapabilities } from '../RelayCapabilities';
import { Choice, Section, StringList } from './Fields';
import { object, strings, type Fields } from './model';
function IdentityFields({
  label,
  value,
  onChange,
}: {
  label: string;
  value: unknown;
  onChange: (v: Fields) => void;
}) {
  const current = object(value);
  return (
    <Section title={label}>
      <Choice
        label={`${label} policy`}
        value={current.type}
        options={['deny', 'open', 'allowlist']}
        onChange={(type) => onChange(type === 'allowlist' ? { type, ids: [] } : { type })}
      />
      {current.type === 'allowlist' ? (
        <StringList
          label={`${label} IDs`}
          value={strings(current.ids)}
          onChange={(ids) => onChange({ ...current, ids })}
        />
      ) : null}
    </Section>
  );
}
export function AdmissionFields({
  value,
  onChange,
  label = 'Admission',
  allowBotMessages = true,
  privateChats = false,
  allowReplyToBot = true,
  allowGroupMentions = true,
}: {
  value: unknown;
  onChange: (v: Fields) => void;
  label?: string;
  allowBotMessages?: boolean;
  privateChats?: boolean;
  allowReplyToBot?: boolean;
  allowGroupMentions?: boolean;
}) {
  const discovery = useRelayCapabilities();
  const admission = object(value);
  return (
    <Section title={label}>
      <Choice
        label={`${label} policy`}
        value={admission.type}
        options={[
          { value: 'all', label: 'All messages' },
          { value: 'allowlist', label: 'Allowed senders' },
          {
            value: 'scoped',
            label: privateChats ? 'Sender and chat rules' : 'Sender, chat and group rules',
          },
        ]}
        onChange={(type) =>
          onChange(
            type === 'scoped'
              ? {
                  type,
                  senders: { type: 'open' },
                  chats: { type: 'open' },
                  groups:
                    privateChats || !allowGroupMentions
                      ? 'deny'
                      : allowReplyToBot
                        ? 'mention_or_reply_to_bot'
                        : 'mention',
                }
              : type === 'allowlist'
                ? { type, sender_ids: [] }
                : { type },
          )
        }
      />
      {admission.type === 'allowlist' ? (
        <StringList
          label="Allowed sender IDs"
          value={strings(admission.sender_ids)}
          onChange={(sender_ids) => onChange({ ...admission, sender_ids })}
        />
      ) : null}
      {admission.type === 'scoped' ? (
        <>
          <IdentityFields
            label="Senders"
            value={admission.senders}
            onChange={(senders) => onChange({ ...admission, senders })}
          />
          <IdentityFields
            label="Chats"
            value={admission.chats}
            onChange={(chats) => onChange({ ...admission, chats })}
          />
          {!privateChats ? (
            <Choice
              label="Group messages"
              value={admission.groups}
              options={
                !allowGroupMentions
                  ? ['deny', 'all']
                  : allowReplyToBot
                    ? ['deny', 'all', 'mention', 'reply_to_bot', 'mention_or_reply_to_bot']
                    : ['deny', 'all', 'mention']
              }
              onChange={(groups) => onChange({ ...admission, groups })}
            />
          ) : null}
          {allowBotMessages ? (
            <Choice
              label="Messages from bots"
              value={admission.bot_messages ?? 'deny'}
              options={[
                { value: 'deny', label: 'Ignore bots' },
                {
                  value: 'native_mentions',
                  label: 'Verified mentions only',
                  ...(discovery.authoring?.native_bot_mentions_writes_enabled === false
                    ? { unavailable: 'New native bot mention writes are disabled on this gateway.' }
                    : {}),
                },
              ]}
              onChange={(bot_messages) => onChange({ ...admission, bot_messages })}
            />
          ) : null}
        </>
      ) : null}
    </Section>
  );
}
