import { nativeWhatsappUnavailable, useRelayCapabilities } from '../RelayCapabilities';
import { AdmissionFields } from './AdmissionFields';
import { Section, StringList, TextField, Toggle } from './Fields';
import { object, strings, type Fields } from './model';

export function WhatsappSourceFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const discovery = useRelayCapabilities();
  const unavailable = nativeWhatsappUnavailable(discovery, false);
  const groups = strings(value.group_ids);
  const groupsEnabled = discovery.authoring?.native_whatsapp_groups_writes_enabled === true;
  const formsEnabled =
    discovery.authoring?.native_whatsapp_forms_enabled === true &&
    discovery.authoring?.native_whatsapp_flow_writes_enabled === true;
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
      {groupsEnabled || groups.length > 0 ? (
        <Section
          title="Existing WhatsApp groups"
          description="Opt in only to existing groups for this Official Business Account phone. CMA checks the phone’s OBA status and current group membership before saving and replying."
        >
          <StringList
            label="Allowed group IDs"
            value={groups}
            max={groupsEnabled ? 16 : groups.length}
            onChange={(group_ids) => {
              const next = { ...value };
              if (group_ids.length) next.group_ids = group_ids;
              else delete next.group_ids;
              onChange(next);
            }}
          />
          <p className="channel-field-hint">
            Use the exact opaque group IDs supplied by Meta. At most 16 groups. Participant rules
            use the original sender; chat rules use the group ID. Scoped admission must also allow
            group messages. Group replies support text, image, PDF, audio, video and approved
            header/body templates.
          </p>
          {!groupsEnabled ? (
            <p className="channel-field-hint">
              New group opt-ins are disabled. Saved groups are retained and can be removed.
            </p>
          ) : null}
        </Section>
      ) : null}
      <Section
        title="Incoming message changes"
        description="Receive private message edits and deletions from WhatsApp Business App coexistence accounts. Meta must support these callbacks for your account."
      >
        <Toggle
          label="Allow coexistence incoming edits and deletions"
          value={value.message_events}
          disabled={
            discovery.authoring?.native_whatsapp_message_events_enabled !== true &&
            value.message_events !== true
          }
          onChange={(message_events) => onChange({ ...value, message_events })}
          hint="Applies to original messages retained after activation. Text and caption edits preserve the original conversation and media. Removing consent stops pending changes."
        />
      </Section>
      <Section
        title="Native forms"
        description="Allow CMA to create and publish managed static WhatsApp Flows with your app credentials for private conversations."
      >
        <Toggle
          label="Allow managed WhatsApp forms"
          value={value.forms_enabled}
          disabled={!formsEnabled && value.forms_enabled !== true}
          onChange={(forms_enabled) => onChange({ ...value, forms_enabled })}
          hint="Forms require a separate preparation step, then an explicit reply within WhatsApp’s 24-hour service window. Disabling consent revokes pending forms and their callbacks."
        />
        {!formsEnabled ? (
          <p className="channel-field-hint">
            New forms are disabled on this gateway. Existing consent can be withdrawn.
          </p>
        ) : null}
      </Section>
      <AdmissionFields
        value={value.admission}
        onChange={(admission) => onChange({ ...value, admission })}
        allowBotMessages={false}
        privateChats={groups.length === 0}
        allowGroupMentions={false}
        allowReplyToBot={false}
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
