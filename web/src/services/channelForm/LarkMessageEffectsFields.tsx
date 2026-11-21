import { useRelayCapabilities } from '../RelayCapabilities';
import { Section, TextField, Toggle } from './Fields';
import { LarkConnectionSelector } from './LarkConnectionSelector';
import { object, optional, text, type Fields } from './model';

export function LarkMessageEffectsFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const allowed = useRelayCapabilities().authoring?.lark_message_effects_writes_enabled === true;
  const enabled = value.lark_message_effects != null;
  const effects = object(value.lark_message_effects);
  const binding = object(effects.connection);
  if (!enabled && !allowed) return null;
  const set = (patch: Fields) =>
    onChange({ ...value, lark_message_effects: { ...effects, ...patch } });
  const connect = (patch: Fields) => set({ connection: { ...binding, ...patch } });
  return (
    <Section title="Manage the bot’s sent messages">
      <Toggle
        label="Allow message effects"
        value={enabled}
        disabled={!allowed && !enabled}
        onChange={(next) =>
          onChange(
            optional(
              value,
              'lark_message_effects',
              next
                ? {
                    connection: { user_service_id: '', catalog_service_id: '', app_id: '' },
                    delete: true,
                    reaction: true,
                  }
                : undefined,
            ),
          )
        }
      />
      {enabled ? (
        <>
          <p className="channel-field-hint">
            Allow agents to delete confirmed bot replies or manage a reaction they added. Applies to
            text and cards outside threads, for 24 hours after sending. Platform permissions and
            recall limits still apply.
          </p>
          <fieldset className="channel-form-section" disabled={!allowed}>
            <Toggle
              label="Delete sent messages"
              value={effects.delete === true}
              onChange={(next) => set({ delete: next })}
            />
            <Toggle
              label="Add, replace and clear the bot’s reaction"
              value={effects.reaction === true}
              onChange={(next) => set({ reaction: next })}
            />
            <LarkConnectionSelector
              platform={value.platform === 'feishu' ? 'feishu' : 'lark'}
              connectionId={text(binding.user_service_id)}
              catalogId={text(binding.catalog_service_id)}
              onSelect={(connection) =>
                connect({
                  user_service_id: connection.user_service_id,
                  catalog_service_id: connection.catalog_service_id,
                })
              }
            >
              <TextField
                label="Message effects connection ID"
                value={binding.user_service_id}
                required
                onChange={(next) => connect({ user_service_id: next })}
              />
              <TextField
                label="Message effects catalog service ID"
                value={binding.catalog_service_id}
                required
                onChange={(next) => connect({ catalog_service_id: next })}
              />
            </LarkConnectionSelector>
            <TextField
              label="Message effects app ID"
              value={binding.app_id}
              required
              onChange={(next) => connect({ app_id: next })}
              hint="Use the cli_ App ID of the same bot configured above, with message read and selected write permissions."
            />
          </fieldset>
          {!allowed ? (
            <p className="channel-field-hint">
              New message effect bindings are disabled on this gateway. You can retain this saved
              binding unchanged or remove it.
            </p>
          ) : null}
        </>
      ) : null}
    </Section>
  );
}
