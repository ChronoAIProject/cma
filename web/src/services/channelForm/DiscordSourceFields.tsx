import { botIdMessage } from './connectorValidation';
import { nativeDiscordUnavailable, useRelayCapabilities } from '../RelayCapabilities';
import { SourceAdmissionFields } from './SourceAdmissionFields';
import { Section, TextField, Toggle } from './Fields';
import { optional, type Fields } from './model';

export function DiscordSourceFields({
  value,
  onChange,
  hideAdmission = false,
}: {
  hideAdmission?: boolean;
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const discovery = useRelayCapabilities();
  const unavailable = nativeDiscordUnavailable(discovery);
  const intents = typeof value.intents === 'number' ? value.intents : 37377;
  const guilds = (intents & 512) !== 0;
  const direct = (intents & 4096) !== 0;
  const reactions = (intents & (1024 | 8192)) !== 0;
  function intent(bits: number, enabled: boolean) {
    let next = (enabled ? intents | bits : intents & ~bits) | 32768;
    next &= ~(1024 | 8192);
    if (reactions) next |= ((next & 512) !== 0 ? 1024 : 0) | ((next & 4096) !== 0 ? 8192 : 0);
    onChange({ ...value, intents: next });
  }
  return (
    <>
      <Section title="Direct Discord bot">
        <p className="channel-field-hint">
          Connect your own Discord bot. Install it in the server, grant access to the intended
          channels and enable Message Content Intent in the Discord Developer Portal. Human messages
          can reach the default consumer; use routing to add named consumers.
        </p>
        {unavailable ? (
          <p role="status" className="channel-field-hint">
            {unavailable} Saved settings are retained.
          </p>
        ) : (
          <p className="channel-field-hint">
            New channels are discovered automatically. Check the saved activation status before
            testing messages; this gateway selects at most{' '}
            {discovery.authoring?.native_discord_capacity ?? 64} native channels.
          </p>
        )}
        <TextField
          path="bot_id"
          validationMessage={botIdMessage(value.bot_id, 'discord')}
          label="Discord bot user ID"
          value={value.bot_id}
          onChange={(bot_id) => onChange({ ...value, bot_id })}
          required
          hint="Use the numeric user ID of the bot whose token you provide."
        />
        <Toggle
          label="Receive server messages"
          value={guilds}
          disabled={guilds && !direct}
          onChange={(enabled) => intent(513, enabled)}
        />
        <Toggle
          label="Receive direct messages"
          value={direct}
          disabled={direct && !guilds}
          onChange={(enabled) => intent(4096, enabled)}
        />
        {discovery.authoring?.native_discord_reactions_enabled === true || reactions ? (
          <>
            <Toggle
              label="Receive reactions to retained messages"
              value={reactions}
              onChange={(enabled) =>
                onChange({
                  ...value,
                  intents: enabled
                    ? intents | (guilds ? 1024 : 0) | (direct ? 8192 : 0)
                    : intents & ~(1024 | 8192),
                })
              }
            />
            <p className="channel-field-hint">
              Reaction changes notify the original consumer. Applies to messages received or sent
              after enabling this option; earlier messages and changed source credentials are
              excluded.
            </p>
          </>
        ) : null}
        {discovery.authoring?.native_discord_thread_lifecycle_enabled === true ||
        value.thread_lifecycle === true ? (
          <>
            <Toggle
              label="Manage threads from incoming messages"
              value={value.thread_lifecycle === true}
              disabled={
                discovery.authoring?.native_discord_thread_lifecycle_enabled !== true &&
                value.thread_lifecycle !== true
              }
              onChange={(enabled) =>
                onChange(optional(value, 'thread_lifecycle', enabled ? true : undefined))
              }
            />
            <p className="channel-field-hint">
              Allow agents to create public threads from incoming server messages, list public
              threads in the same parent channel, and rename, archive, reopen, join or leave those
              exact threads as the bot. Messages already in a thread stay within that thread.
              Discord permissions still apply. Renaming requires an active thread and never reopens
              it. Private thread creation and forum posts are not supported.
            </p>
          </>
        ) : null}
        {discovery.authoring?.native_discord_conversation_history_enabled === true ||
        value.conversation_history === true ? (
          <>
            <Toggle
              label="Read earlier conversation messages"
              value={value.conversation_history === true}
              disabled={
                discovery.authoring?.native_discord_conversation_history_enabled !== true &&
                value.conversation_history !== true
              }
              onChange={(enabled) =>
                onChange(optional(value, 'conversation_history', enabled ? true : undefined))
              }
            />
            <p className="channel-field-hint">
              Allow agents to page backward through earlier messages from other participants and
              bots in the exact conversation that received a new human message. Includes text and
              attachment details, without file downloads. Threads stay within that thread. Discord
              permissions and content access can limit results; this does not provide a complete
              archive. Applies only to new messages received after enabling this option.
            </p>
          </>
        ) : null}
        {discovery.authoring?.native_discord_thread_history_enabled === true ||
        value.thread_history === true ? (
          <>
            <Toggle
              label="Read earlier thread messages"
              value={value.thread_history === true}
              disabled={
                discovery.authoring?.native_discord_thread_history_enabled !== true &&
                value.thread_history !== true
              }
              onChange={(enabled) =>
                onChange(optional(value, 'thread_history', enabled ? true : undefined))
              }
            />
            <p className="channel-field-hint">
              Allow the agent to read up to 20 earlier messages in the same thread when needed. This
              does not include other channels or older history.
            </p>
          </>
        ) : null}
        <p className="channel-field-hint">
          Message Content Intent is required. Bot messages are not supported. Native buttons and
          single-select menus require operator enablement. Thread sessions require verified native
          channel metadata enabled by the operator; otherwise thread partitioning is refused.
        </p>
      </Section>
      {!hideAdmission ? <SourceAdmissionFields value={value} onChange={onChange} /> : null}
    </>
  );
}
