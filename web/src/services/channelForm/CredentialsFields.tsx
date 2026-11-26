import { Button } from '../../ui/Button';
import { Section, TextField, Toggle } from './Fields';
import { records, type Fields } from './model';
import { hmacCredentialIdMessage, hmacSecretMessage } from './hmacCredentials';
const labels: Record<string, string> = {
  agent_key: 'Provider agent key',
  channel_key: 'NyxID channel key',
  telegram_bot_token: 'Telegram bot token',
  discord_bot_token: 'Discord bot token',
  telegram_webhook_secret: 'Telegram webhook secret',
};
export function CredentialsFields({
  value,
  onChange,
  keys,
  create = false,
  hmac = false,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
  keys: readonly string[];
  create?: boolean;
  hmac?: boolean;
}) {
  const rows = records(value.hmac_secrets);
  return (
    <Section title="Private credentials">
      <p className="channel-field-hint">
        {create
          ? 'Credentials are sent securely when you create this channel.'
          : 'Leave fields empty to keep saved credentials. Enter a value only to replace it.'}{' '}
        Saved secret values are never displayed.
      </p>
      {keys.map((key) => (
        <TextField
          key={key}
          label={labels[key] ?? key}
          value={value[key]}
          onChange={(v) => onChange({ ...value, [key]: v })}
          secret
          required={create && key === 'agent_key'}
          hint={
            key === 'agent_key'
              ? 'Authorizes this channel to send work to the selected provider.'
              : undefined
          }
        />
      ))}
      {hmac ? (
        <>
          {create ? (
            <p className="channel-field-hint">
              Set the initial HMAC credentials for this channel. Each HMAC producer must use one of
              these key IDs.
            </p>
          ) : (
            <Toggle
              label="Replace HMAC credentials"
              value={value.replace_hmac}
              onChange={(replace_hmac) => onChange({ ...value, replace_hmac })}
            />
          )}
          {create || value.replace_hmac === true ? (
            <>
              {!create ? (
                <p className="channel-field-hint">
                  This replaces the whole HMAC key list. Include every key you want to keep.
                </p>
              ) : null}
              {rows.map((row, index) => (
                <Section key={index} title={`HMAC credential ${index + 1}`}>
                  <TextField
                    path={`/credentials/hmac_secrets/${index}/key_id`}
                    label={`HMAC ${index + 1} key ID`}
                    value={row.key_id}
                    validationMessage={hmacCredentialIdMessage(row.key_id, rows, index)}
                    onChange={(key_id) =>
                      onChange({
                        ...value,
                        hmac_secrets: rows.map((v, i) => (i === index ? { ...v, key_id } : v)),
                      })
                    }
                    required
                  />
                  <TextField
                    path={`/credentials/hmac_secrets/${index}/secret`}
                    label={`HMAC ${index + 1} secret`}
                    value={row.secret}
                    validationMessage={hmacSecretMessage(row.secret)}
                    hint="Standard base64, including required = padding, up to 4,096 characters. No spaces or line breaks."
                    onChange={(secret) =>
                      onChange({
                        ...value,
                        hmac_secrets: rows.map((v, i) => (i === index ? { ...v, secret } : v)),
                      })
                    }
                    secret
                    required
                  />
                  <Button
                    onClick={() =>
                      onChange({ ...value, hmac_secrets: rows.filter((_, i) => i !== index) })
                    }
                  >
                    Remove credential {index + 1}
                  </Button>
                </Section>
              ))}
              <Button
                onClick={() =>
                  onChange({ ...value, hmac_secrets: [...rows, { key_id: '', secret: '' }] })
                }
              >
                Add HMAC credential
              </Button>
            </>
          ) : null}
        </>
      ) : null}
    </Section>
  );
}
