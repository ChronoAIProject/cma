import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { ServiceError, serviceRequest, type Denied } from './client';

export interface RelayAuthoringCapabilities {
  readonly native_discord_writes_enabled?: boolean | null;
  readonly native_discord_auto_activation_enabled?: boolean | null;
  readonly native_discord_capacity?: number | null;
  readonly dynamic_sources_writes_enabled: boolean;
  readonly relay_raw_verification_writes_enabled: boolean;
  readonly lark_resource_writes_enabled?: boolean | null;
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
  readonly retry?: () => void;
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
  const pending =
    useRef<Promise<{ relay_authoring?: RelayAuthoringCapabilities | null }>>(undefined);
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
          setDiscovery(
            value.relay_authoring
              ? { status: 'ready', authoring: value.relay_authoring }
              : { status: 'legacy' },
          );
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
