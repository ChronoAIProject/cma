import { Section, Toggle } from './Fields';
import { optional, strings, type Fields } from './model';
import { useRelayCapabilities } from '../RelayCapabilities';

import { telegramMemberJoinGroupIds } from './telegramMemberJoins';

export function TelegramMemberJoinFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const discovery = useRelayCapabilities();
  const enabled = discovery.authoring?.native_telegram_member_joins_enabled === true;
  const selected = strings(value.member_join_chat_ids);
  if (!enabled && selected.length === 0) return null;
  const allowed = value.message_events === true ? telegramMemberJoinGroupIds(value) : [];
  const groups = enabled ? [...new Set([...allowed, ...selected])] : selected;
  return (
    <Section title="Welcome new Telegram group members" path="member_join_chat_ids">
      <p className="channel-field-hint">
        Each join update sends one group event to routing targets that accept non-human activity.
        Bot-only joins are ignored. Replies go to the group.
      </p>
      {groups.length === 0 ? (
        <p className="channel-field-hint">
          Enable message edits and reactions, allow all group messages and senders, and list the
          exact Telegram group IDs under Chat admission.
        </p>
      ) : null}
      {groups.map((id) => (
        <Toggle
          key={id}
          label={`Welcome new members in ${id}`}
          value={selected.includes(id)}
          disabled={
            !selected.includes(id) && (!enabled || !allowed.includes(id) || selected.length >= 16)
          }
          onChange={(checked) => {
            const next = checked ? [...selected, id] : selected.filter((chat) => chat !== id);
            onChange(optional(value, 'member_join_chat_ids', next));
          }}
        />
      ))}
    </Section>
  );
}
