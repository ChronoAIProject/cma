import { Button } from '../../ui/Button';
import { Choice, Section, StringList, TextField, Toggle } from './Fields';
import { object, optional, strings, type Fields } from './model';
import { useValidationPath } from './ValidationPath';
import { useHmacCredentials } from './HmacCredentialsScope';
import { hmacReferenceMessage } from './hmacCredentials';

export function CustomSourceFields({
  value,
  onChange,
  attachmentDescription,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
  attachmentDescription?: string;
}) {
  const auth = object(value.auth),
    attachments = object(value.attachments);
  const set = (key: string, next: unknown) => onChange(optional(value, key, next));
  const schemaPath = useValidationPath('schema');
  const credentials = useHmacCredentials();
  return (
    <>
      <Section
        title="Custom event authentication"
        description="Choose how your application authenticates incoming HTTP events. The prompt template controls how event data is presented to the agent."
      >
        <Choice
          label="Authentication method"
          value={auth.type}
          options={[
            { value: 'hmac', label: 'HMAC signature' },
            { value: 'nyxid_key', label: 'NyxID agent key' },
          ]}
          onChange={(type) =>
            set('auth', type === 'hmac' ? { type, key_id: '' } : { type, key_hashes: [] })
          }
        />
        {auth.type === 'hmac' ? (
          <TextField
            path="auth/key_id"
            label="HMAC key ID"
            hint="Use the exact key ID from your HMAC credentials."
            validationMessage={
              credentials
                ? hmacReferenceMessage(auth.key_id, credentials.value, credentials.create)
                : undefined
            }
            value={auth.key_id}
            onChange={(key_id) => set('auth', { ...auth, key_id })}
            required
          />
        ) : (
          <StringList
            label="Allowed key hashes"
            value={strings(auth.key_hashes)}
            onChange={(key_hashes) => set('auth', { ...auth, key_hashes })}
          />
        )}
        <TextField
          label="Prompt template"
          value={value.prompt_template}
          onChange={(v) => set('prompt_template', v || undefined)}
          multiline
        />
      </Section>
      <Section
        title="Event attachments"
        description={
          attachmentDescription ??
          'Allow attachments from approved HTTPS origins. What the agent can open depends on its connection and attachment support.'
        }
      >
        <Toggle
          label="Accept event attachments"
          value={value.attachments != null}
          onChange={(enabled) =>
            set('attachments', enabled ? { pointer: '/attachments', origins: [] } : undefined)
          }
        />
        {value.attachments != null ? (
          <>
            <TextField
              label="Attachment list path"
              value={attachments.pointer}
              onChange={(pointer) => set('attachments', { ...attachments, pointer })}
              required
              hint="Path inside event data, for example /attachments."
            />
            <StringList
              label="Allowed HTTPS origins"
              value={strings(attachments.origins)}
              onChange={(origins) => set('attachments', { ...attachments, origins })}
              max={16}
            />
          </>
        ) : null}
      </Section>
      {value.schema != null ? (
        // The v1 gateway contract refuses every Custom event schema, so none is offered.
        <Section
          title="Event data validation"
          description="Event schema validation is not supported by this gateway. Remove it before saving."
        >
          <div data-validation-path={schemaPath}>
            <Button onClick={() => set('schema', undefined)}>Remove schema validation</Button>
          </div>
        </Section>
      ) : null}
    </>
  );
}
