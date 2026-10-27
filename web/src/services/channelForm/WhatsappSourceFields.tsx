import { nativeWhatsappUnavailable, useRelayCapabilities } from '../RelayCapabilities';
import { AdmissionFields } from './AdmissionFields';
import { Section, TextField } from './Fields';
import { object, type Fields } from './model';

export function WhatsappSourceFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const unavailable = nativeWhatsappUnavailable(useRelayCapabilities(), false);
  return (
    <>
      <Section
        title="WhatsApp Business account"
        description="Connect a dedicated Meta app to receive private messages and reactions."
      >
        <TextField
          label="Meta app ID"
          value={value.app_id}
          onChange={(app_id) => onChange({ ...value, app_id })}
          required
        />
        <TextField
          label="WhatsApp Business account ID"
          value={value.waba_id}
          onChange={(waba_id) => onChange({ ...value, waba_id })}
          required
        />
        <TextField
          label="Phone number ID"
          value={value.phone_number_id}
          onChange={(phone_number_id) => onChange({ ...value, phone_number_id })}
          hint="Use Meta’s numeric phone number ID, rather than the phone number."
          required
        />
        <p className="channel-field-hint">
          After saving, copy this producer’s WhatsApp webhook URL into the Meta app, use the same
          verification token, and subscribe to messages. Use a dedicated app: changing its webhook
          also changes delivery for any other integration using that app.
        </p>
        {unavailable ? (
          <p className="channel-field-hint">{unavailable} Saved settings are retained.</p>
        ) : null}
      </Section>
      <AdmissionFields
        value={value.admission}
        onChange={(admission) => onChange({ ...value, admission })}
        allowBotMessages={false}
        privateChats
      />
    </>
  );
}

export function WhatsappCredentialsFields({
  value,
  onChange,
  create = false,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
  create?: boolean;
}) {
  const credentials = object(value.whatsapp);
  return (
    <Section title="WhatsApp credentials">
      <p className="channel-field-hint">
        {create
          ? 'Enter all three credentials for this Meta app.'
          : 'Leave all three fields empty to keep saved credentials. To replace them, enter all three.'}{' '}
        Saved secret values are never displayed. CMA verifies the app, account and phone before
        saving.
      </p>
      {[
        ['app_secret', 'Meta app secret'],
        ['access_token', 'Meta access token'],
        ['verify_token', 'Webhook verification token'],
      ].map(([key, label]) => (
        <TextField
          key={key}
          label={label!}
          value={credentials[key!]}
          onChange={(next) => onChange({ ...value, whatsapp: { ...credentials, [key!]: next } })}
          hint={
            key === 'access_token'
              ? 'Needs WhatsApp Business management and messaging permissions.'
              : undefined
          }
          secret
          required={create}
        />
      ))}
    </Section>
  );
}
