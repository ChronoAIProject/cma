import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { ServiceError, serviceRequest, type Denied } from './client';

export interface RelayAuthoringCapabilities {
  readonly custom_durable_execution_enabled?: boolean | null;
  readonly response_recovery_providers?: readonly string[] | null;
  readonly native_lark_writes_enabled?: boolean | null;
  readonly native_lark_intake_enabled?: boolean | null;
  readonly native_lark_controls_enabled?: boolean | null;
  readonly native_lark_forms_enabled?: boolean | null;
  readonly native_whatsapp_forms_enabled?: boolean | null;
  readonly native_whatsapp_message_events_enabled?: boolean | null;
  readonly native_whatsapp_flow_writes_enabled?: boolean | null;
  readonly native_whatsapp_writes_enabled?: boolean | null;
  readonly native_whatsapp_groups_writes_enabled?: boolean | null;
  readonly native_whatsapp_auto_activation_enabled?: boolean | null;
  readonly native_telegram_message_events_enabled?: boolean | null;
  readonly native_telegram_member_joins_enabled?: boolean | null;
  readonly relay_telegram_member_joins_enabled?: boolean | null;
  readonly native_discord_thread_history_enabled?: boolean | null;
  readonly native_telegram_topic_lifecycle_enabled?: boolean | null;
  readonly native_telegram_topic_creation_enabled?: boolean | null;
  readonly native_telegram_general_topic_lifecycle_enabled?: boolean | null;
  readonly native_discord_thread_lifecycle_enabled?: boolean | null;
  readonly native_discord_conversation_history_enabled?: boolean | null;
  readonly native_lark_conversation_history_enabled?: boolean | null;
  readonly native_discord_reactions_enabled?: boolean | null;
  readonly native_discord_writes_enabled?: boolean | null;
  readonly native_discord_auto_activation_enabled?: boolean | null;
  readonly native_discord_capacity?: number | null;
  readonly dynamic_sources_writes_enabled: boolean;
  readonly relay_raw_verification_writes_enabled: boolean;
  readonly lark_resource_writes_enabled?: boolean | null;
  readonly lark_message_effects_writes_enabled?: boolean | null;
  readonly outbound_messages_writes_enabled?: boolean | null;
  readonly native_media_writes_enabled?: boolean | null;
  readonly multi_consumer_writes_enabled: boolean;
  readonly native_bot_mentions_writes_enabled: boolean;
  readonly thread_contract_writes_enabled: boolean;
  readonly platforms: readonly {
    readonly id: string;
    readonly label: string;
    readonly verification_modes: readonly {
      readonly id: string;
      readonly writes_enabled: boolean;
    }[];
  }[];
}
export interface RelayDiscovery {
  readonly status: 'loading' | 'ready' | 'legacy' | 'failed';
  readonly authoring?: RelayAuthoringCapabilities;
  readonly managedTelegramSetup?: boolean;
  readonly agentTools?: readonly AgentToolCapability[];
  readonly retry?: () => void;
}
export interface AgentToolCapability {
  readonly name: string;
  readonly description: string;
  readonly read_only: boolean;
  readonly destructive: boolean;
  readonly idempotent: boolean;
  readonly open_world: boolean;
}
interface CapabilityView {
  readonly platforms?: readonly {
    readonly id: string;
    readonly connection_methods: readonly string[];
  }[];
  readonly relay_authoring?: RelayAuthoringCapabilities | null;
  readonly agent_tools?: readonly AgentToolCapability[] | null;
}
export const RelayCapabilitiesContext = createContext<RelayDiscovery>({ status: 'legacy' });
export const useRelayCapabilities = () => useContext(RelayCapabilitiesContext);

/** One discovery per mounted owner/permission context. Navigation does not refetch. */
export function RelayCapabilitiesProvider({
  children,
  denied,
}: {
  children: ReactNode;
  denied: Denied;
}) {
  const [discovery, setDiscovery] = useState<RelayDiscovery>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);
  const pending = useRef<Promise<CapabilityView>>(undefined);
  const deniedRef = useRef(denied);
  useEffect(() => {
    deniedRef.current = denied;
  }, [denied]);
  useEffect(() => {
    let active = true;
    pending.current ??= serviceRequest('event-gateway', '/v1/channel-capabilities');
    void pending.current.then(
      (value) => {
        if (active)
          setDiscovery({
            status: value.relay_authoring ? 'ready' : 'legacy',
            authoring: value.relay_authoring ?? undefined,
            managedTelegramSetup:
              value.platforms?.some(
                (platform) =>
                  platform.id === 'telegram' &&
                  platform.connection_methods.includes('managed_create'),
              ) === true,
            agentTools: value.agent_tools ?? undefined,
          });
      },
      (error: unknown) => {
        if (!active) return;
        if (error instanceof ServiceError && [401, 403].includes(error.status))
          deniedRef.current(error.status);
        setDiscovery({
          status: error instanceof ServiceError && error.status === 404 ? 'legacy' : 'failed',
        });
      },
    );
    return () => {
      active = false;
    };
  }, [attempt]);
  const retry = () => {
    pending.current = undefined;
    setDiscovery({ status: 'loading' });
    setAttempt((value) => value + 1);
  };
  return (
    <RelayCapabilitiesContext value={{ ...discovery, retry }}>{children}</RelayCapabilitiesContext>
  );
}

export function modeUnavailable(
  discovery: RelayDiscovery,
  platform: string,
  mode: string,
): string | undefined {
  if (discovery.authoring) {
    const available = discovery.authoring.platforms
      .find((v) => v.id === platform)
      ?.verification_modes.find((v) => v.id === mode);
    if (!available) return 'This gateway does not advertise this verification mode.';
    if (!available.writes_enabled) return 'New writes for this mode are disabled on this gateway.';
    return undefined;
  }
  if (mode !== 'raw_event') return undefined;
  return discovery.status === 'loading'
    ? 'Checking gateway support…'
    : 'This gateway has not confirmed support for new raw-event verification writes.';
}

export function RelayDiscoveryNotice() {
  const discovery = useRelayCapabilities();
  if (discovery.status === 'ready') return null;
  return (
    <p className="channel-field-hint" role="status">
      {discovery.status === 'loading'
        ? 'Checking available verification modes…'
        : 'Existing settings are retained. Raw-event verification needs capability confirmation from this gateway.'}
      {discovery.status === 'failed' && discovery.retry ? (
        <Button onClick={discovery.retry}>Retry capability check</Button>
      ) : null}
    </p>
  );
}

/** Native authoring and automatic intake are distinct, explicitly advertised gates. */
export function nativeDiscordUnavailable(
  discovery: RelayDiscovery,
  automatic = true,
): string | undefined {
  if (discovery.authoring?.native_discord_writes_enabled !== true)
    return discovery.status === 'loading'
      ? 'Checking direct Discord support…'
      : 'This gateway has not enabled direct Discord authoring.';
  if (
    automatic &&
    (discovery.authoring.native_discord_auto_activation_enabled !== true ||
      discovery.authoring.multi_consumer_writes_enabled !== true)
  )
    return 'This gateway has not enabled automatic activation for new direct Discord channels.';
  return undefined;
}

/** Additive discovery: older gateways still enforce eligibility when saving. */
export function nativeDiscordProviderUnavailable(
  discovery: RelayDiscovery,
  provider: string,
): string | undefined {
  const supported = discovery.authoring?.response_recovery_providers;
  if (!provider || supported == null || supported.includes(provider)) return undefined;
  return `${provider} does not support reconnecting to interrupted Discord responses on this gateway yet. Choose another agent provider before creating a direct Discord channel.`;
}

export function nativeWhatsappUnavailable(
  discovery: RelayDiscovery,
  automatic = true,
): string | undefined {
  if (discovery.authoring?.native_whatsapp_writes_enabled !== true)
    return discovery.status === 'loading'
      ? 'Checking direct WhatsApp support…'
      : 'This gateway has not enabled direct WhatsApp setup.';
  if (automatic && discovery.authoring.native_whatsapp_auto_activation_enabled !== true)
    return 'This gateway has not enabled message delivery for new direct WhatsApp producers.';
  return undefined;
}

export function nativeLarkUnavailable(
  discovery: RelayDiscovery,
  intake = true,
): string | undefined {
  if (discovery.authoring?.native_lark_writes_enabled !== true)
    return discovery.status === 'loading'
      ? 'Checking direct Lark / Feishu support…'
      : 'This gateway has not enabled direct Lark / Feishu setup.';
  if (intake && discovery.authoring.native_lark_intake_enabled !== true)
    return 'This gateway has not enabled message delivery for new direct Lark / Feishu producers.';
  return undefined;
}
