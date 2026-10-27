import type { Denied } from '../client';
import { nativeDiscordUnavailable, useRelayCapabilities } from '../RelayCapabilities';
import { ManagedConnectionFields } from './ManagedConnectionFields';
import { Button } from '../../ui/Button';
import { ChannelPolicyFields } from './ChannelPolicyFields';
import { CredentialsFields } from './CredentialsFields';
import { Choice, Section } from './Fields';
import { ProfileFields, ProviderFields } from './ProfileFields';
import { SourceFields } from './SourceFields';
import { defaultSource, object, records, type EditorSection, type Fields } from './model';
export function NodeFields({
  draft,
  onChange,
  credentials,
  onCredentials,
  section,
  create,
  sourceCount,
  denied,
}: {
  draft: Fields;
  onChange: (v: Fields) => void;
  credentials: Fields;
  onCredentials: (v: Fields) => void;
  section: EditorSection;
  create: boolean;
  sourceCount: number;
  denied: Denied;
}) {
  const discovery = useRelayCapabilities();
  const sources = records(draft.sources);
  if (!create && section.kind === 'producer' && section.index === sources.length)
    sources.push(defaultSource('nyxid_relay'));
  const producer = section.kind === 'producer' ? section.index : -1;
  const set = (key: string, next: unknown) => onChange({ ...draft, [key]: next });
  return (
    <>
      {create || section.kind === 'channel' ? (
        <ChannelPolicyFields value={draft} onChange={onChange} hideConnection={create} />
      ) : null}
      {create ? (
        <ManagedConnectionFields value={draft} onChange={onChange} denied={denied} />
      ) : null}
      {create || section.kind === 'producer'
        ? sources.map((source, index) =>
            create || index === producer ? (
              <Section title={`Producer ${index + 1}`} key={index}>
                {create || index >= sourceCount ? (
                  <Choice
                    label={`Producer ${index + 1} connection type`}
                    value={source.type}
                    options={[
                      { value: 'nyxid_relay', label: 'NyxID messaging connection' },
                      { value: 'telegram', label: 'Direct Telegram bot' },
                      {
                        value: 'discord',
                        label: 'Direct Discord bot',
                        unavailable: nativeDiscordUnavailable(discovery),
                      },
                      { value: 'custom', label: 'Custom event' },
                    ]}
                    onChange={(type) =>
                      set(
                        'sources',
                        sources.map((v, i) => (i === index ? defaultSource(type) : v)),
                      )
                    }
                  />
                ) : null}
                <SourceFields
                  value={source}
                  onChange={(next) =>
                    set(
                      'sources',
                      sources.map((v, i) => (i === index ? next : v)),
                    )
                  }
                />
                {!create ? (
                  <CredentialsFields
                    value={credentials}
                    onChange={onCredentials}
                    keys={
                      source.type === 'discord'
                        ? ['discord_bot_token']
                        : source.type === 'telegram'
                          ? ['telegram_bot_token', 'telegram_webhook_secret']
                          : source.type === 'nyxid_relay'
                            ? ['channel_key']
                            : []
                    }
                    hmac={source.type === 'custom'}
                  />
                ) : null}
              </Section>
            ) : null,
          )
        : null}
      {create && draft.connection_ref === undefined ? (
        <Button
          disabled={sources.length >= 4}
          onClick={() => set('sources', [...sources, defaultSource('custom')])}
        >
          Add producer
        </Button>
      ) : null}
      {create || (section.kind === 'consumer' && section.id === 'default') ? (
        <>
          <ProviderFields
            immutable={!create}
            value={object(draft.provider)}
            onChange={(v) => set('provider', v)}
          />
          <ProfileFields
            immutableIdentity={!create}
            value={object(draft.profile)}
            onChange={(v) => set('profile', v)}
          />
          <CredentialsFields
            value={credentials}
            onChange={onCredentials}
            keys={
              create && draft.connection_ref === undefined
                ? [
                    'agent_key',
                    'channel_key',
                    'telegram_bot_token',
                    'telegram_webhook_secret',
                    ...(sources.some((source) => source.type === 'discord')
                      ? ['discord_bot_token']
                      : []),
                  ]
                : ['agent_key']
            }
            create={create}
            hmac={create}
          />
        </>
      ) : null}
      {!create && section.kind === 'channel' ? (
        <CredentialsFields value={credentials} onChange={onCredentials} keys={[]} hmac />
      ) : null}
      {!create && section.kind === 'producer' && !sources[section.index] ? (
        <p>This producer is no longer attached. Select another node.</p>
      ) : null}
    </>
  );
}
export function nodeCredentials(values: Fields, section: EditorSection): Fields {
  const keys =
    section.kind === 'consumer'
      ? ['agent_key']
      : section.kind === 'channel'
        ? ['replace_hmac', 'hmac_secrets']
        : [
            'channel_key',
            'discord_bot_token',
            'telegram_bot_token',
            'telegram_webhook_secret',
            'replace_hmac',
            'hmac_secrets',
          ];
  return Object.fromEntries(Object.entries(values).filter(([key]) => keys.includes(key)));
}
