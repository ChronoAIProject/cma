import { LarkResourceFields } from './LarkResourceFields';
import { DiscordSourceFields } from './DiscordSourceFields';
import { Button } from '../../ui/Button';
import { modeUnavailable, RelayDiscoveryNotice, useRelayCapabilities } from '../RelayCapabilities';
import { RelaySetupGuidance } from './RelaySetupGuidance';
import { AdmissionFields } from './AdmissionFields';
import { Choice, Section, StringList, TextField, Toggle } from './Fields';
import { object, optional, strings, without, type Fields } from './model';
import { ValueBuilder } from './ValueBuilder';
const platforms = ['telegram', 'lark', 'feishu', 'discord', 'whatsapp', 'slack'];
export function SourceFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (v: Fields) => void;
}) {
  const discovery = useRelayCapabilities();
  const rawUnavailable = modeUnavailable(discovery, String(value.platform), 'raw_event');
  const trustedUnavailable = modeUnavailable(
    discovery,
    String(value.platform),
    'trusted_normalized',
  );
  const set = (key: string, next: unknown) => onChange({ ...value, [key]: next });
  if (value.type === 'discord') return <DiscordSourceFields value={value} onChange={onChange} />;
  if (value.type === 'custom') return <CustomSource value={value} onChange={onChange} />;
  if (value.type !== 'telegram' && value.type !== 'nyxid_relay')
    return (
      <p>
        This producer uses a configuration version this editor does not support. Its settings will
        be retained.
      </p>
    );
  return (
    <>
      <Section title={value.type === 'telegram' ? 'Telegram bot' : 'NyxID connection'}>
        {value.type === 'nyxid_relay' ? (
          <>
            <Choice
              label="Messaging platform"
              value={value.platform}
              options={platforms.map((platform) => ({
                value: platform,
                label: platform,
                ...(discovery.authoring &&
                !discovery.authoring.platforms.some((v) => v.id === platform)
                  ? { unavailable: 'Not supported by this gateway.' }
                  : {}),
              }))}
              onChange={(platform) => {
                const updated = {
                  ...without(value, platform !== value.platform ? ['lark_resources'] : []),
                  platform,
                  ...(['discord', 'whatsapp'].includes(platform) &&
                  !value.trust_normalized &&
                  !modeUnavailable(discovery, platform, 'raw_event')
                    ? { verify_raw_event: true }
                    : {}),
                };
                onChange(
                  ['discord', 'whatsapp'].includes(platform)
                    ? updated
                    : without(
                        updated,
                        platform === 'slack'
                          ? ['verify_raw_event']
                          : ['verify_raw_event', 'trust_normalized'],
                      ),
                );
              }}
            />
            <TextField
              label="NyxID issuer"
              value={value.issuer}
              onChange={(v) => set('issuer', v)}
              required
            />
            <TextField
              label="Signing key ID"
              value={value.key_id}
              onChange={(v) => set('key_id', v)}
              required
            />
            <StringList
              label="Route IDs"
              value={strings(value.route_ids)}
              onChange={(v) => set('route_ids', v)}
            />
            {['discord', 'whatsapp'].includes(String(value.platform)) ? (
              <Choice
                label="Incoming message verification"
                value={
                  value.verify_raw_event ? 'verified' : value.trust_normalized ? 'trusted' : ''
                }
                options={[
                  {
                    value: '',
                    label: 'Choose verification',
                    unavailable: 'Select an explicit policy.',
                  },
                  {
                    value: 'verified',
                    label: 'Verify original platform event',
                    unavailable: rawUnavailable,
                  },
                  {
                    value: 'trusted',
                    label: 'Trust normalized NyxID messages',
                    unavailable: trustedUnavailable,
                  },
                ]}
                onChange={(mode) =>
                  onChange({
                    ...without(value, ['trust_normalized', 'verify_raw_event']),
                    ...(mode === 'verified'
                      ? { verify_raw_event: true }
                      : { trust_normalized: true }),
                  })
                }
              />
            ) : ['slack'].includes(String(value.platform)) ? (
              <Toggle
                label="Trust normalized NyxID messages"
                value={value.trust_normalized}
                disabled={Boolean(trustedUnavailable) && value.trust_normalized !== true}
                onChange={(v) => set('trust_normalized', v)}
              />
            ) : null}
            <RelayDiscoveryNotice />
            {value.verify_raw_event === true && rawUnavailable ? (
              <p className="channel-field-hint">
                The saved or drafted raw verification setting is retained. This gateway must enable
                writes before it can be added or changed; there is no automatic switch to normalized
                trust.
              </p>
            ) : null}
            {value.trust_normalized === true ? (
              <p className="channel-field-hint">
                This explicitly trusts NyxID's normalized message fields instead of checking the
                original platform event.
              </p>
            ) : null}
            <RelaySetupGuidance platform={String(value.platform)} />
          </>
        ) : null}
        <TextField
          label="Bot ID"
          value={value.bot_id}
          onChange={(v) => onChange(optional(value, 'bot_id', v || undefined))}
          required={value.type === 'telegram'}
          hint="Use the platform's bot user ID for exact mention and reply matching."
        />
        <TextField
          label="Bot username"
          value={value.bot_username}
          onChange={(v) => onChange(optional(value, 'bot_username', v || undefined))}
        />
      </Section>
      {value.type === 'nyxid_relay' && ['lark', 'feishu'].includes(String(value.platform)) ? (
        <LarkResourceFields value={value} onChange={onChange} />
      ) : null}
      <AdmissionFields value={value.admission} onChange={(v) => set('admission', v)} />
    </>
  );
}
function CustomSource({ value, onChange }: { value: Fields; onChange: (v: Fields) => void }) {
  const auth = object(value.auth),
    attachments = object(value.attachments);
  const set = (key: string, next: unknown) => onChange(optional(value, key, next));
  return (
    <>
      <Section title="Custom event authentication">
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
            label="HMAC key ID"
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
      <Section title="Event attachments">
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
      <Section title="Event data validation">
        {value.schema != null ? (
          <>
            <ValueBuilder
              label="Event schema"
              value={value.schema}
              onChange={(v) => set('schema', v)}
            />
            <Button onClick={() => set('schema', undefined)}>Remove schema validation</Button>
          </>
        ) : (
          <Button onClick={() => set('schema', { type: 'object', properties: {} })}>
            Add schema validation
          </Button>
        )}
      </Section>
    </>
  );
}
