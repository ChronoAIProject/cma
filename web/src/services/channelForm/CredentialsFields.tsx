import { Button } from '../../ui/Button';
import { Section, TextField, Toggle } from './Fields';
import { records, type Fields } from './model';
const labels: Record<string, string> = {
  agent_key: 'Provider agent key',
  channel_key: 'NyxID channel key',
  telegram_bot_token: 'Telegram bot token',
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
        />
      ))}
      {hmac ? (
        <>
          <Toggle
            label="Replace HMAC credentials"
            value={value.replace_hmac}
            onChange={(replace_hmac) => onChange({ ...value, replace_hmac })}
          />
          {value.replace_hmac === true ? (
            <>
              <p className="channel-field-hint">
                This replaces the whole HMAC key list. Include every key you want to keep.
              </p>
              {rows.map((row, index) => (
                <Section key={index} title={`HMAC credential ${index + 1}`}>
                  <TextField
                    label={`HMAC ${index + 1} key ID`}
                    value={row.key_id}
                    onChange={(key_id) =>
                      onChange({
                        ...value,
                        hmac_secrets: rows.map((v, i) => (i === index ? { ...v, key_id } : v)),
                      })
                    }
                    required
                  />
                  <TextField
                    label={`HMAC ${index + 1} secret`}
                    value={row.secret}
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
