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
}: {
  value: unknown;
  onChange: (v: Fields) => void;
  label?: string;
}) {
  const admission = object(value);
  return (
    <Section title={label}>
      <Choice
        label={`${label} policy`}
        value={admission.type}
        options={[
          { value: 'all', label: 'All messages' },
          { value: 'allowlist', label: 'Allowed senders' },
          { value: 'scoped', label: 'Sender, chat and group rules' },
        ]}
        onChange={(type) =>
          onChange(
            type === 'scoped'
              ? {
                  type,
                  senders: { type: 'open' },
                  chats: { type: 'open' },
                  groups: 'mention_or_reply_to_bot',
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
          <Choice
            label="Group messages"
            value={admission.groups}
            options={['deny', 'all', 'mention', 'reply_to_bot', 'mention_or_reply_to_bot']}
            onChange={(groups) => onChange({ ...admission, groups })}
          />
          <Choice
            label="Messages from bots"
            value={admission.bot_messages ?? 'deny'}
            options={[
              { value: 'deny', label: 'Ignore bots' },
              { value: 'native_mentions', label: 'Verified mentions only' },
            ]}
            onChange={(bot_messages) => onChange({ ...admission, bot_messages })}
          />
        </>
      ) : null}
    </Section>
  );
}
