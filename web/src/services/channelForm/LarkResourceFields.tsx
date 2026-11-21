import { useRelayCapabilities } from '../RelayCapabilities';
import { Section, TextField, Toggle } from './Fields';
import { LarkConnectionSelector } from './LarkConnectionSelector';
import { object, optional, text, type Fields } from './model';

export function LarkResourceFields({
  value,
  onChange,
}: {
  value: Fields;
  onChange: (value: Fields) => void;
}) {
  const allowed = useRelayCapabilities().authoring?.lark_resource_writes_enabled === true;
  const enabled = value.lark_resources != null;
  const binding = object(value.lark_resources);
  if (!enabled && !allowed) return null;
  const set = (field: string, next: string) =>
    onChange({ ...value, lark_resources: { ...binding, [field]: next } });
  const identifiers = (
    <>
      <TextField
        label="NyxID service connection ID"
        value={binding.user_service_id}
        onChange={(next) => set('user_service_id', next)}
        required
      />
      <TextField
        label="NyxID catalog service ID"
        value={binding.catalog_service_id}
        onChange={(next) => set('catalog_service_id', next)}
        required
        hint={
          value.platform === 'feishu'
            ? 'Use the api-feishu-bot catalog service.'
            : 'Use the api-lark-bot catalog service.'
        }
      />
    </>
  );
  return (
    <Section title="Embedded images and videos · advanced setup">
      <Toggle
        label="Read media in rich-text posts"
        value={enabled}
        onChange={(next) =>
          onChange(
            optional(
              value,
              'lark_resources',
              next ? { user_service_id: '', catalog_service_id: '', app_id: '' } : undefined,
            ),
          )
        }
        disabled={!allowed && !enabled}
      />
      {enabled ? (
        <>
          <p className="channel-field-hint">
            Choose an existing NyxID bot API connection for this same receiving bot. Its Bot ID
            above must be the independently confirmed open_id. The existing channel key must have
            access to this exact connection.
          </p>
          <fieldset className="channel-form-section" disabled={!allowed}>
            {allowed ? (
              <LarkConnectionSelector
                key={text(value.platform)}
                platform={value.platform === 'feishu' ? 'feishu' : 'lark'}
                connectionId={text(binding.user_service_id)}
                catalogId={text(binding.catalog_service_id)}
                onSelect={(connection) =>
                  onChange({
                    ...value,
                    lark_resources: {
                      ...binding,
                      user_service_id: connection.user_service_id,
                      catalog_service_id: connection.catalog_service_id,
                    },
                  })
                }
              >
                {identifiers}
              </LarkConnectionSelector>
            ) : (
              identifiers
            )}
            <TextField
              label="Receiving app ID"
              value={binding.app_id}
              onChange={(next) => set('app_id', next)}
              required
              hint="The cli_ App ID of this relay bot, not its open_id."
            />
          </fieldset>
          {!allowed ? (
            <p className="channel-field-hint">
              This gateway retains the saved binding but has not enabled new resource bindings. You
              can remove it.
            </p>
          ) : null}
        </>
      ) : null}
    </Section>
  );
}
