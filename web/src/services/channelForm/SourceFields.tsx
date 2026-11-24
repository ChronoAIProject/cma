import { CustomSourceFields } from './CustomSourceFields';
import { botIdMessage, telegramUsernameMessage } from './connectorValidation';
import { AdvancedFields } from './AdvancedFields';
import { LarkMessageEffectsFields } from './LarkMessageEffectsFields';
import { LarkResourceFields } from './LarkResourceFields';
import { DiscordSourceFields } from './DiscordSourceFields';
import { NativeLarkSourceFields } from './NativeLarkSourceFields';
import { WhatsappSourceFields } from './WhatsappSourceFields';
import { modeUnavailable, RelayDiscoveryNotice, useRelayCapabilities } from '../RelayCapabilities';
import { RelaySetupGuidance } from './RelaySetupGuidance';
import { SourceAdmissionFields } from './SourceAdmissionFields';
import { TelegramMemberJoinFields } from './TelegramMemberJoinFields';
import { Choice, Section, StringList, TextField, Toggle } from './Fields';
import { optional, strings, without, type Fields } from './model';
const platforms = ['telegram', 'lark', 'feishu', 'discord', 'whatsapp', 'slack'];
export function SourceFields({
  value,
  onChange,
  hideAdmission = false,
  managedIdentity = false,
  guided = false,
  attachmentDescription,
}: {
  hideAdmission?: boolean;
  value: Fields;
  onChange: (v: Fields) => void;
  managedIdentity?: boolean;
  guided?: boolean;
  attachmentDescription?: string;
}) {
  const discovery = useRelayCapabilities();
  const rawUnavailable = modeUnavailable(discovery, String(value.platform), 'raw_event');
  const trustedUnavailable = modeUnavailable(
    discovery,
    String(value.platform),
    'trusted_normalized',
  );
  const set = (key: string, next: unknown) => onChange({ ...value, [key]: next });
  if (value.type === 'native_lark')
    return (
      <NativeLarkSourceFields value={value} onChange={onChange} hideAdmission={hideAdmission} />
    );
  if (value.type === 'discord')
    return <DiscordSourceFields value={value} onChange={onChange} hideAdmission={hideAdmission} />;
  if (value.type === 'whatsapp')
    return <WhatsappSourceFields value={value} onChange={onChange} hideAdmission={hideAdmission} />;
  if (value.type === 'custom')
    return (
      <CustomSourceFields
        value={value}
        onChange={onChange}
        attachmentDescription={attachmentDescription}
      />
    );
  if (value.type !== 'telegram' && value.type !== 'nyxid_relay')
    return (
      <p>
        This producer uses a configuration version this editor does not support. Its settings will
        be retained.
      </p>
    );
  return (
    <>
      <Section
        title={value.type === 'telegram' ? 'Telegram bot' : 'NyxID connection'}
        description="Identify the bot and connection that send messages into this channel."
      >
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
                  ...without(
                    value,
                    platform !== value.platform ? ['lark_resources', 'member_join_chat_ids'] : [],
                  ),
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
            {guided ? (
              <p className="channel-field-hint channel-connection-summary">
                Issuer: {String(value.issuer || 'Required')} · Signing key:{' '}
                {String(value.key_id || 'Required')} · Routes:{' '}
                {strings(value.route_ids).filter(Boolean).join(', ') || 'None entered'}
              </p>
            ) : null}
            <AdvancedFields title="Advanced connection settings" enabled={guided}>
              <TextField
                path="issuer"
                label="NyxID issuer"
                value={value.issuer}
                onChange={(v) => set('issuer', v)}
                hint="Copy this value from your existing NyxID relay route."
                required
              />
              <TextField
                path="key_id"
                label="Signing key ID"
                value={value.key_id}
                onChange={(v) => set('key_id', v)}
                hint="Copy this value from your existing NyxID relay route."
                required
              />
              <StringList
                requiredItems
                path="route_ids"
                label="Route IDs"
                value={strings(value.route_ids)}
                onChange={(v) => set('route_ids', v)}
              />
              <p className="channel-field-hint">
                Use exact route IDs to receive callbacks. Removing every route saves an unbound
                source that admits no callbacks.
              </p>
            </AdvancedFields>
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
            {value.platform === 'telegram' ? (
              <TelegramMemberJoinFields value={value} onChange={onChange} />
            ) : null}
            <RelaySetupGuidance platform={String(value.platform)} />
          </>
        ) : null}
        {value.type === 'telegram' ? (
          <>
            <Toggle
              label="Manage the original forum topic"
              value={value.topic_lifecycle === true}
              disabled={
                discovery.authoring?.native_telegram_topic_lifecycle_enabled !== true &&
                value.topic_lifecycle !== true
              }
              onChange={(enabled) =>
                onChange(optional(value, 'topic_lifecycle', enabled || undefined))
              }
            />
            <p className="channel-field-hint">
              Agents may rename, close or reopen the forum topic containing an admitted message.
              Requires a forum supergroup and bot administrator permission to manage topics.
              {discovery.authoring?.native_telegram_general_topic_lifecycle_enabled === true
                ? ' General topics are included; reopening General also makes it visible. '
                : ' General topics are unavailable. '}
              Private chats and topic browsing are excluded; creation needs separate consent.
            </p>
            <Toggle
              label="Create new forum topics"
              value={value.topic_creation === true}
              disabled={
                discovery.authoring?.native_telegram_topic_creation_enabled !== true &&
                value.topic_creation !== true
              }
              onChange={(enabled) =>
                onChange(optional(value, 'topic_creation', enabled || undefined))
              }
            />
            <p className="channel-field-hint">
              Agents may create empty named topics in the forum supergroup containing an admitted
              human message, then rename, close or reopen the topics they created. Requires bot
              administrator permission to manage topics. Private chats, browsing and icon selection
              are unavailable. Existing topic management is separate.
            </p>
            <Toggle
              label="Receive message edits and reactions"
              value={value.message_events}
              disabled={
                discovery.authoring?.native_telegram_message_events_enabled !== true &&
                value.message_events !== true
              }
              onChange={(enabled) =>
                onChange(
                  enabled
                    ? optional(value, 'message_events', true)
                    : without(value, ['message_events', 'channel_posts', 'member_join_chat_ids']),
                )
              }
            />
            <p className="channel-field-hint">
              Private and group message edits and reactions from identifiable users reach the agents
              that received the original message.{' '}
              {managedIdentity
                ? 'Managed connections register these event types; an older registration may need an operator refresh. '
                : 'If the current webhook does not subscribe to these event types, refresh its registration. '}
              Reaction updates require bot administrator access. Anonymous reactions and aggregate
              reaction counts keep the actor unknown and require open sender admission. Telegram
              does not provide general message deletion events.
            </p>
            <Toggle
              label="Receive channel posts and edits"
              value={value.channel_posts}
              disabled={
                value.message_events !== true ||
                (discovery.authoring?.native_telegram_message_events_enabled !== true &&
                  value.channel_posts !== true)
              }
              onChange={(enabled) =>
                onChange(optional(value, 'channel_posts', enabled || undefined))
              }
            />
            <p className="channel-field-hint">
              Requires message events and open sender admission. Channel posts identify the channel,
              not a human publisher; chat allowlists and addressing rules still apply. Channel edits
              reach the original agents. Discussion-group forwards and channel direct-message topics
              are not included.{' '}
              {managedIdentity
                ? 'Managed connections register these event types; an older registration may need an operator refresh.'
                : 'If the current webhook does not subscribe to these event types, refresh its registration.'}
            </p>
            <TelegramMemberJoinFields value={value} onChange={onChange} />
            {discovery.authoring?.native_telegram_message_events_enabled !== true ? (
              <p className="channel-field-hint">
                This gateway has not enabled new message event settings. Saved settings are
                retained.
              </p>
            ) : null}
          </>
        ) : null}
        {managedIdentity ? (
          <p className="channel-field-hint">
            Bot {String(value.bot_id)}
            {value.bot_username ? ` (@${String(value.bot_username)})` : ''} is managed by this
            connection. Its identity and credentials are retained.
          </p>
        ) : (
          <>
            <TextField
              path="bot_id"
              validationMessage={
                value.type === 'telegram' ? botIdMessage(value.bot_id, 'telegram') : undefined
              }
              label="Bot ID"
              value={value.bot_id}
              onChange={(v) => onChange(optional(value, 'bot_id', v || undefined))}
              required={value.type === 'telegram'}
              hint="Use the platform's bot user ID for exact mention and reply matching."
            />
            <TextField
              path="bot_username"
              validationMessage={
                value.type === 'telegram' ? telegramUsernameMessage(value.bot_username) : undefined
              }
              hint={
                value.type === 'telegram'
                  ? 'Optional: 5–32 letters, numbers or underscores, without @.'
                  : undefined
              }
              label="Bot username"
              value={value.bot_username}
              onChange={(v) => onChange(optional(value, 'bot_username', v || undefined))}
            />
          </>
        )}
      </Section>
      {value.type === 'nyxid_relay' && ['lark', 'feishu'].includes(String(value.platform)) ? (
        <>
          <LarkResourceFields value={value} onChange={onChange} />
          <LarkMessageEffectsFields value={value} onChange={onChange} />
        </>
      ) : null}
      {!hideAdmission ? <SourceAdmissionFields value={value} onChange={onChange} /> : null}
    </>
  );
}
