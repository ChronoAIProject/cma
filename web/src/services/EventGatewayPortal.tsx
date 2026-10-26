import { useState, type ReactNode } from 'react';
import { BotManagement, canManageBots, isBotRoute } from '../BotManagement';
import { useCmaClient } from '../api/context';
import { useAuth } from '../auth/context';
import type { BotRoute } from '../bots/BotPage';
import { retainFatherDraftForSignIn } from '../bots/fatherDraft';
import { ShellLoading } from '../ui/Skeleton';
import { SignInLanding } from '../landing/SignInLanding';
import { ReferenceProvider } from '../profiles/ReferenceLink';
import { routePath, type AppRoute } from '../routing/routes';
import { PortalFrame } from '../shell/PortalFrame';
import { SIDEBAR_AUTO_COLLAPSE } from '../shell/columns';
import { EventGateway } from './EventGateway';
import { ServiceBody } from './ServiceBody';
import { ServicePage } from './ServicePage';
import './EventGateway.css';
import { GatewayConsumers } from './GatewayConsumers';
import { GatewayWelcome } from './GatewayWelcome';

/** Keep channel editors and consumer forms mounted when switching Gateway sections. */
export function EventGatewayPortal({
  route,
  onNavigate,
  visible,
  sidebarCollapsed,
  onSidebarCollapsedChange,
}: {
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
  readonly visible: boolean;
  readonly sidebarCollapsed: boolean;
  readonly onSidebarCollapsedChange: (collapsed: boolean) => void;
}) {
  const { state } = useAuth();
  const client = useCmaClient();
  const consumers = isBotRoute(route);
  const [visitedChannels, setVisitedChannels] = useState(!consumers);
  const [consumerRoute, setConsumerRoute] = useState<BotRoute | undefined>(
    consumers ? route : undefined,
  );
  if (!consumers && !visitedChannels) setVisitedChannels(true);
  if (consumers && consumerRoute !== route) setConsumerRoute(route);
  const canManage = state.status === 'authenticated' && canManageBots(state.user, client);
  const [channelsExpanded, setChannelsExpanded] = useState(true);
  const [consumersExpanded, setConsumersExpanded] = useState(true);
  const navigation = (selected: 'channels' | 'consumers', channels?: ReactNode) => (
    <nav className="gateway-navigation" aria-label="Event Gateway navigation">
      <button
        type="button"
        className="sidebar__link"
        aria-current={selected === 'channels' ? 'page' : undefined}
        aria-expanded={selected === 'channels' && channelsExpanded}
        onClick={() => {
          if (selected === 'channels') setChannelsExpanded((value) => !value);
          else {
            setChannelsExpanded(true);
            onNavigate({ kind: 'event_gateway' });
          }
        }}
      >
        <span>Channels</span>
        <span aria-hidden="true">{selected === 'channels' && channelsExpanded ? '⌄' : '›'}</span>
      </button>
      <div hidden={selected !== 'channels' || !channelsExpanded}>{channels}</div>
      {canManage ? (
        <>
          <button
            type="button"
            className="sidebar__link"
            aria-current={selected === 'consumers' ? 'page' : undefined}
            aria-expanded={selected === 'consumers' && consumersExpanded}
            onClick={() => {
              if (selected === 'consumers') setConsumersExpanded((value) => !value);
              else {
                setConsumersExpanded(true);
                onNavigate({ kind: 'bots' });
              }
            }}
          >
            <span>Consumers</span>
            <span aria-hidden="true">
              {selected === 'consumers' && consumersExpanded ? '⌄' : '›'}
            </span>
          </button>
          {selected === 'consumers' && client.botViews ? (
            <div hidden={!consumersExpanded}>
              <GatewayConsumers
                key={state.status === 'authenticated' ? state.user.id : ''}
                client={client.botViews}
                selected={consumerRoute?.kind === 'bot' ? consumerRoute.botId : undefined}
                onNavigate={onNavigate}
              />
            </div>
          ) : null}
        </>
      ) : null}
    </nav>
  );
  const frame = (content: ReactNode, channels?: ReactNode) => (
    <PortalFrame
      fullWidth
      route={{ kind: 'event_gateway' }}
      onNavigate={onNavigate}
      navigation={navigation('channels', channels)}
      sidebarCollapsed={sidebarCollapsed}
      onSidebarCollapsedChange={onSidebarCollapsedChange}
    >
      {content}
    </PortalFrame>
  );
  return (
    <>
      {visitedChannels ? (
        <ServiceBody visible={visible && !consumers}>
          <ServicePage service="event-gateway" renderStatus={(content) => frame(content)}>
            {(session, denied, notice) => (
              <EventGateway
                session={session}
                denied={denied}
                active={visible && !consumers}
                onNavigate={() => {
                  if (window.innerWidth < SIDEBAR_AUTO_COLLAPSE) onSidebarCollapsedChange(true);
                }}
              >
                {({ sidebar, content }) =>
                  frame(
                    <>
                      {notice}
                      {content}
                    </>,
                    sidebar,
                  )
                }
              </EventGateway>
            )}
          </ServicePage>
        </ServiceBody>
      ) : null}
      {consumerRoute ? (
        <ServiceBody visible={visible && consumers}>
          <PortalFrame
            fullWidth
            route={consumerRoute}
            onNavigate={onNavigate}
            navigation={navigation('consumers')}
            sidebarCollapsed={sidebarCollapsed}
            onSidebarCollapsedChange={onSidebarCollapsedChange}
          >
            {consumerRoute.kind === 'bots' && canManage ? (
              <GatewayWelcome kind="consumer" onCreate={() => onNavigate({ kind: 'bot_new' })} />
            ) : (
              <Consumers route={consumerRoute} onNavigate={onNavigate} />
            )}
          </PortalFrame>
        </ServiceBody>
      ) : null}
    </>
  );
}

function Consumers({
  route,
  onNavigate,
}: {
  readonly route: BotRoute;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const { state, signIn } = useAuth();
  if (state.status === 'checking') return <ShellLoading label="Signing you in" />;
  if (state.status === 'error') return <p role="alert">{state.message}</p>;
  if (state.status === 'anonymous')
    return (
      <SignInLanding
        onSignIn={async () => {
          retainFatherDraftForSignIn();
          await signIn(routePath(route));
        }}
      />
    );
  return (
    <ReferenceProvider key={state.user.id} onNavigate={onNavigate}>
      <BotManagement user={state.user} route={route} onNavigate={onNavigate} />
    </ReferenceProvider>
  );
}
