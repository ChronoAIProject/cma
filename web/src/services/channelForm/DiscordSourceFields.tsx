import { nativeDiscordUnavailable, useRelayCapabilities } from '../RelayCapabilities';
import { AdmissionFields } from './AdmissionFields';
import { Section, TextField, Toggle } from './Fields';
import type { Fields } from './model';

export function DiscordSourceFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const discovery = useRelayCapabilities();
  const unavailable = nativeDiscordUnavailable(discovery);
  const intents = typeof value.intents === 'number' ? value.intents : 37377;
  const guilds = (intents & 512) !== 0;
  const direct = (intents & 4096) !== 0;
  function intent(bits: number, enabled: boolean) {
    onChange({ ...value, intents: (enabled ? intents | bits : intents & ~bits) | 32768 });
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
        <p className="channel-field-hint">
          Message Content Intent is required. Bot messages are not supported. Native buttons and
          single-select menus require operator enablement and are unavailable in threads. Thread
          sessions require verified native channel metadata enabled by the operator; otherwise
          thread partitioning is refused.
        </p>
      </Section>
      <AdmissionFields
        value={value.admission}
        onChange={(admission) => onChange({ ...value, admission })}
        allowBotMessages={false}
      />
    </>
  );
}
