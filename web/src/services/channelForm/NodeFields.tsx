import { useRef } from 'react';
import { ValidationScope } from './ValidationPath';
import { SetupPolicy, SetupReview, SetupSection } from './SetupSections';
import type { Denied } from '../client';
import {
  nativeDiscordProviderUnavailable,
  nativeDiscordUnavailable,
  nativeLarkUnavailable,
  nativeWhatsappUnavailable,
  useRelayCapabilities,
} from '../RelayCapabilities';
import { NativeLarkCredentialsFields } from './NativeLarkSourceFields';
import { WhatsappCredentialsFields } from './WhatsappSourceFields';
import { ManagedConnectionFields } from './ManagedConnectionFields';
import { Button } from '../../ui/Button';
import { ChannelPolicyFields } from './ChannelPolicyFields';
import { CredentialsFields } from './CredentialsFields';
import { fitPartition } from './authoringRules';
import { Choice, Section } from './Fields';
import { ProfileFields, ProviderFields } from './ProfileFields';
import { SourceFields } from './SourceFields';
import {
  customAttachmentDescription,
  customExecutionSelected,
  hasCustomSource,
} from './customExecution';
import {
  customUsesHmac,
  defaultSource,
  object,
  records,
  producerCredentialKeys,
  usesHmacCredentials,
  type EditorSection,
  type Fields,
} from './model';
export function NodeFields({
  draft,
  active = true,
  onChange,
  credentials,
  onCredentials,
  section,
  create,
  sourceCount,
  savedCustomExecution,
  retainedAckDelay,
  denied,
}: {
  draft: Fields;
  active?: boolean;
  onChange: (v: Fields) => void;
  credentials: Fields;
  onCredentials: (v: Fields) => void;
  section: EditorSection;
  create: boolean;
  sourceCount: number;
  savedCustomExecution?: boolean;
  retainedAckDelay?: unknown;
  denied: Denied;
}) {
  const discovery = useRelayCapabilities();
  const sourceDrafts = useRef(new Map<number, Map<string, Fields>>());
  const sources = records(draft.sources);
  if (!create && section.kind === 'producer' && section.index === sources.length)
    sources.push(defaultSource('nyxid_relay'));
  const producer = section.kind === 'producer' ? section.index : -1;
  const providerUnavailable = nativeDiscordProviderUnavailable(
    discovery,
    String(object(draft.provider).slug ?? ''),
  );
  const set = (key: string, next: unknown) => {
    const value = { ...draft, [key]: next };
    // A new channel's grouping follows its producers; saved channels are never rewritten.
    onChange(create && key === 'sources' ? fitPartition(discovery, value) : value);
  };
  function removeProducer(index: number) {
    const drafts = sourceDrafts.current;
    drafts.delete(index);
    for (let next = index + 1; next < sources.length; next++) {
      const retained = drafts.get(next);
      drafts.delete(next);
      if (retained) drafts.set(next - 1, retained);
    }
    set(
      'sources',
      sources.filter((_, i) => i !== index),
    );
  }
  return (
    <>
      <SetupSection
        enabled={create}
        title="Connection"
        description="Choose a supported platform and a connection you own. Next, review Agent behavior, access and replies. Fields marked Required must be completed before saving."
      >
        {create || section.kind === 'channel' ? (
          <ChannelPolicyFields
            value={draft}
            onChange={onChange}
            hideConnection={create}
            retainedAckDelay={retainedAckDelay}
            part={create ? 'identity' : 'all'}
          />
        ) : null}
        {create &&
        (discovery.managedTelegramSetup === true || draft.connection_ref !== undefined) ? (
          <ManagedConnectionFields
            active={active}
            value={draft}
            onChange={onChange}
            denied={denied}
            hideAdmission
          />
        ) : null}
        {create && hasCustomSource(draft) && discovery.status === 'failed' ? (
          <p className="channel-field-hint" role="status">
            Could not check channel support. Your draft is retained.
            {discovery.retry ? (
              <Button onClick={discovery.retry}>Retry support check</Button>
            ) : null}
          </p>
        ) : null}
        {create || section.kind === 'producer'
          ? sources.map((source, index) =>
              create || index === producer ? (
                <Section title={`Producer ${index + 1}`} key={index} navigation={false}>
                  {create || index >= sourceCount ? (
                    <Choice
                      label={`Producer ${index + 1} connection type`}
                      value={source.type}
                      options={[
                        { value: 'nyxid_relay', label: 'NyxID messaging connection' },
                        { value: 'telegram', label: 'Direct Telegram bot' },
                        {
                          value: 'native_lark',
                          label: 'Direct Lark / Feishu app',
                          unavailable:
                            nativeLarkUnavailable(discovery) ??
                            (sources.some((other, i) => i !== index && other.type === 'native_lark')
                              ? 'A channel can have only one direct Lark / Feishu producer.'
                              : undefined),
                        },
                        {
                          value: 'whatsapp',
                          label: 'Direct WhatsApp Business',
                          unavailable: nativeWhatsappUnavailable(discovery),
                        },
                        {
                          value: 'discord',
                          label: 'Direct Discord bot',
                          unavailable: nativeDiscordUnavailable(discovery) ?? providerUnavailable,
                        },
                        { value: 'custom', label: 'Custom event' },
                      ].filter((option) => {
                        // Hide new setup until the gateway confirms activation. Existing
                        // sources below remain inspectable when rollout gates are paused.
                        if (option.value === 'native_lark')
                          return !nativeLarkUnavailable(discovery);
                        if (option.value === 'whatsapp')
                          return !nativeWhatsappUnavailable(discovery);
                        if (option.value === 'discord') return !nativeDiscordUnavailable(discovery);
                        return true;
                      })}
                      onChange={(type) => {
                        const drafts = sourceDrafts.current.get(index) ?? new Map<string, Fields>();
                        drafts.set(String(source.type), source);
                        sourceDrafts.current.set(index, drafts);
                        set(
                          'sources',
                          sources.map((v, i) =>
                            i === index ? (drafts.get(type) ?? defaultSource(type)) : v,
                          ),
                        );
                      }}
                    />
                  ) : null}
                  <ValidationScope value={`/sources/${index}`}>
                    <SourceFields
                      guided={create}
                      hideAdmission={create}
                      value={source}
                      attachmentDescription={customAttachmentDescription(
                        draft,
                        create
                          ? customExecutionSelected(discovery, draft)
                          : savedCustomExecution === true,
                      )}
                      managedIdentity={Boolean(draft.connection_ref)}
                      onChange={(next) =>
                        set(
                          'sources',
                          sources.map((v, i) => (i === index ? next : v)),
                        )
                      }
                    />
                  </ValidationScope>
                  {source.type === 'native_lark' ? (
                    <NativeLarkCredentialsFields
                      value={credentials}
                      onChange={onCredentials}
                      create={create || index >= sourceCount}
                    />
                  ) : null}
                  {source.type === 'whatsapp' ? (
                    <WhatsappCredentialsFields
                      value={credentials}
                      onChange={onCredentials}
                      create={create}
                    />
                  ) : null}
                  {!create &&
                  !draft.connection_ref &&
                  !['whatsapp', 'native_lark'].includes(String(source.type)) ? (
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
                      hmac={customUsesHmac(source)}
                    />
                  ) : null}
                  {create && index > 0 ? (
                    <Button onClick={() => removeProducer(index)}>
                      Remove producer {index + 1}
                    </Button>
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
        {create && (draft.connection_ref === undefined || usesHmacCredentials(draft)) ? (
          <CredentialsFields
            value={credentials}
            onChange={onCredentials}
            create
            keys={(draft.connection_ref === undefined ? sources : [])
              .flatMap(producerCredentialKeys)
              .filter(
                (key) => !['native_lark', 'whatsapp', 'replace_hmac', 'hmac_secrets'].includes(key),
              )
              .filter((key, index, keys) => keys.indexOf(key) === index)}
            hmac={usesHmacCredentials(draft)}
          />
        ) : null}
      </SetupSection>
      <SetupSection
        enabled={create}
        title="Agent behavior"
        description="Review the destination, model, instructions and service permissions used for incoming messages."
      >
        {create || (section.kind === 'consumer' && section.id === 'default') ? (
          <>
            {create &&
            sources.some((source) => source.type === 'discord') &&
            providerUnavailable ? (
              <p className="channel-field-hint" role="status">
                {providerUnavailable}
              </p>
            ) : null}
            <ProviderFields
              immutable={!create}
              value={object(draft.provider)}
              onChange={(v) => set('provider', v)}
            />
            <ProfileFields
              guided={create}
              immutableIdentity={!create}
              value={object(draft.profile)}
              onChange={(v) => set('profile', v)}
            />
            <CredentialsFields
              value={credentials}
              onChange={onCredentials}
              keys={['agent_key']}
              create={create}
            />
          </>
        ) : null}
      </SetupSection>
      {create ? (
        <>
          <SetupPolicy value={draft} onChange={onChange} />
          <SetupReview value={draft} />
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
export function nodeCredentials(values: Fields, section: EditorSection, draft: Fields): Fields {
  if (section.kind === 'producer' && draft.connection_ref) return {};
  const keys =
    section.kind === 'consumer'
      ? ['agent_key']
      : section.kind === 'channel'
        ? ['replace_hmac', 'hmac_secrets']
        : producerCredentialKeys(object(records(draft.sources)[section.index]));
  return Object.fromEntries(Object.entries(values).filter(([key]) => keys.includes(key)));
}
