import { useState, type ReactNode } from 'react';
import { BotManagement, canManageBots, isBotRoute } from '../BotManagement';
import { useCmaClient } from '../api/context';
import { useAuth } from '../auth/context';
import type { BotRoute } from '../bots/BotPage';
import { retainFatherDraftForSignIn } from '../bots/fatherDraft';
import { BrandLoading } from '../landing/LandingCharacter';
import { SignInLanding } from '../landing/SignInLanding';
import { ReferenceProvider } from '../profiles/ReferenceLink';
import { routePath, type AppRoute } from '../routing/routes';
import { PortalFrame } from '../shell/PortalFrame';
import { SIDEBAR_AUTO_COLLAPSE } from '../shell/columns';
import { EventGateway } from './EventGateway';
import { ServiceBody } from './ServiceBody';
import { ServicePage } from './ServicePage';
import './EventGateway.css';

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
  const navigation = (selected: 'channels' | 'consumers') => (
    <nav className="gateway-navigation" aria-label="Event Gateway navigation">
      {(
        [
          ['channels', 'Channels', { kind: 'event_gateway' }],
          ...(canManage ? [['consumers', 'Consumers', { kind: 'bots' }] as const] : []),
        ] as const
      ).map(([id, label, next]) => (
        <a
          key={id}
          href={routePath(next)}
          aria-current={selected === id ? 'page' : undefined}
          onClick={(event) => {
            if (
              event.button !== 0 ||
              event.metaKey ||
              event.ctrlKey ||
              event.shiftKey ||
              event.altKey
            )
              return;
            event.preventDefault();
            if (window.innerWidth < SIDEBAR_AUTO_COLLAPSE) onSidebarCollapsedChange(true);
            onNavigate(next);
          }}
        >
          {label}
        </a>
      ))}
    </nav>
  );
  const frame = (content: ReactNode, channels?: ReactNode) => (
    <PortalFrame
      route={{ kind: 'event_gateway' }}
      onNavigate={onNavigate}
      navigation={
        <>
          {navigation('channels')}
          {channels}
        </>
      }
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
            route={consumerRoute}
            onNavigate={onNavigate}
            navigation={navigation('consumers')}
            sidebarCollapsed={sidebarCollapsed}
            onSidebarCollapsedChange={onSidebarCollapsedChange}
          >
            <Consumers route={consumerRoute} onNavigate={onNavigate} />
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
  if (state.status === 'checking') return <BrandLoading label="Signing you in" />;
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
