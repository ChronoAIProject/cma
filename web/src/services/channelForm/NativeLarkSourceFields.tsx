import { larkIdentityMessage } from './connectorValidation';
import { nativeLarkUnavailable, useRelayCapabilities } from '../RelayCapabilities';
import { SourceAdmissionFields } from './SourceAdmissionFields';
import { Choice, Section, TextField, Toggle } from './Fields';
import { object, optional, type Fields } from './model';

export function NativeLarkSourceFields({
  value,
  onChange,
  hideAdmission = false,
}: {
  hideAdmission?: boolean;
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const identity = object(value.identity);
  const discovery = useRelayCapabilities();
  const unavailable = nativeLarkUnavailable(discovery, false);
  const setIdentity = (key: string, next: string) =>
    onChange({ ...value, identity: { ...identity, [key]: next } });
  return (
    <>
      <Section
        title="Lark / Feishu app"
        description="Receive text messages, recalls and human reaction changes through a separate custom app."
      >
        <Choice
          label="App region"
          value={identity.region}
          options={[
            { value: 'lark', label: 'Lark (international)' },
            { value: 'feishu', label: 'Feishu (China)' },
          ]}
          onChange={(region) => setIdentity('region', region)}
        />
        <TextField
          path="identity/app_id"
          validationMessage={larkIdentityMessage(identity.app_id, 'app_id')}
          label="App ID"
          value={identity.app_id}
          onChange={(next) => setIdentity('app_id', next)}
          required
          hint="Use the separate app’s cli_ identifier from the selected region."
        />
        <TextField
          path="identity/tenant_key"
          validationMessage={larkIdentityMessage(identity.tenant_key, 'tenant_key')}
          label="Tenant key"
          value={identity.tenant_key}
          onChange={(next) => setIdentity('tenant_key', next)}
          required
          hint="The exact tenant in which this app is installed."
        />
        <TextField
          path="identity/bot_open_id"
          validationMessage={larkIdentityMessage(identity.bot_open_id, 'bot_open_id')}
          label="Bot open ID"
          value={identity.bot_open_id}
          onChange={(next) => setIdentity('bot_open_id', next)}
          required
          hint="Use this app’s ou_ bot identity. Identities from another app cannot be reused."
        />
        <Toggle
          path="separate_app"
          label="This is a separate app with its own webhook"
          value={value.separate_app}
          onChange={(separate_app) => onChange({ ...value, separate_app })}
          hint="Lark / Feishu allows one request URL per app. Keep existing NyxID app webhooks unchanged."
        />
        <p className="channel-field-hint">
          Enable the bot, publish and install this app in the selected tenant, and add it to the
          conversations it should receive. After saving, copy its Lark / Feishu webhook URL into
          this app and configure the same Encrypt Key and Verification Token.
        </p>
        <p className="channel-field-hint">
          Subscribe to im.message.receive_v1, im.message.recalled_v1, im.message.reaction.created_v1
          and im.message.reaction.deleted_v1. Grant the app receive, recall and reaction-read
          permissions for those conversations, and send-message permission for text replies. Saving
          verifies app and bot identity; it does not configure subscriptions.
        </p>
        <p className="channel-field-hint">
          {discovery.authoring?.native_lark_controls_enabled === true
            ? 'Interactive cards, buttons and choices are enabled. In the separate app’s Callback configuration, subscribe to card.action.trigger only and use the same encrypted webhook URL. This subscription is separate from message events.'
            : 'Interactive card, button and choice replies are disabled on this gateway.'}{' '}
          {discovery.authoring?.native_lark_forms_enabled === true
            ? 'Form submissions are enabled for the original person in the same conversation or existing group thread.'
            : 'Form replies are disabled.'}
        </p>
        {discovery.authoring?.native_lark_conversation_history_enabled === true ||
        value.conversation_history === true ? (
          <div>
            <Toggle
              label="Allow original conversation history"
              value={value.conversation_history === true}
              onChange={(enabled) =>
                onChange(optional(value, 'conversation_history', enabled ? true : undefined))
              }
            />
            <p className="channel-field-hint">
              Allow earlier messages from other participants and apps in the original chat or exact
              thread, according to this bot’s visibility. Thread reads may need several empty scan
              pages. Grant im:message:readonly and, for groups, im:message.group_msg. Enabling
              applies to newly received events.
            </p>
          </div>
        ) : (
          <p className="channel-field-hint">
            Original conversation history is disabled on this gateway.
          </p>
        )}
        {unavailable ? (
          <p className="channel-field-hint">{unavailable} Saved settings are retained.</p>
        ) : null}
      </Section>
      {!hideAdmission ? <SourceAdmissionFields value={value} onChange={onChange} /> : null}
    </>
  );
}

export function NativeLarkCredentialsFields({
  value,
  onChange,
  create = false,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
  create?: boolean;
}) {
  const credentials = object(value.native_lark);
  return (
    <Section title="Lark / Feishu credentials">
      <p className="channel-field-hint">
        {create
          ? 'Enter all three credentials for this separate app.'
          : 'Leave all three fields empty to keep saved credentials. To replace them, enter all three.'}{' '}
        Saved secrets are never displayed. Each value must contain 16–512 printable ASCII characters
        without spaces.
      </p>
      {[
        ['app_secret', 'App secret'],
        ['encrypt_key', 'Encrypt Key'],
        ['verification_token', 'Verification Token'],
      ].map(([key, label]) => (
        <TextField
          key={key}
          label={label!}
          value={credentials[key!]}
          onChange={(next) => onChange({ ...value, native_lark: { ...credentials, [key!]: next } })}
          secret
          required={create}
        />
      ))}
    </Section>
  );
}
