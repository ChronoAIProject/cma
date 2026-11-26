import { useState, type ReactNode } from 'react';
import { BotManagement, canManageBots, isBotRoute } from '../BotManagement';
import { useCmaClient } from '../api/context';
import { useAuth } from '../auth/context';
import { IdentityError } from '../auth/IdentityNotice';
import type { BotRoute } from '../bots/BotPage';
import { retainFatherDraftForSignIn } from '../bots/fatherDraft';
import { ShellLoading } from '../ui/Skeleton';
import { SignInLanding } from '../landing/SignInLanding';
import { ReferenceProvider } from '../profiles/ReferenceLink';
import { routePath, type AppRoute } from '../routing/routes';
import { PortalFrame } from '../shell/PortalFrame';
import { SIDEBAR_AUTO_COLLAPSE } from '../shell/columns';
import { NavigationViews } from '../shell/NavigationViews';
import type { SidebarNewAgent } from '../shell/Sidebar';
import { IconSparkle16 } from '../ui/icons';
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
  const { state, signIn, explicitlySignedOut } = useAuth();
  const client = useCmaClient();
  const consumers = isBotRoute(route);
  const [visitedChannels, setVisitedChannels] = useState(!consumers);
  const [consumerRoute, setConsumerRoute] = useState<BotRoute | undefined>(
    consumers ? route : undefined,
  );
  if (!consumers && !visitedChannels) setVisitedChannels(true);
  if (consumers && consumerRoute !== route) setConsumerRoute(route);
  const canManage = state.status === 'authenticated' && canManageBots(state.user, client);
  const navigation = (selected: 'channels' | 'consumers', channels?: ReactNode) => (
    <nav className="gateway-navigation" aria-label="Event Gateway navigation">
      <div className="navigation-filters">
        <div className="navigation-filters__bar">
          <NavigationViews
            value={selected}
            options={
              canManage
                ? [
                    { value: 'channels', label: 'Channels' },
                    { value: 'consumers', label: 'Consumers' },
                  ]
                : [{ value: 'channels', label: 'Channels' }]
            }
            onChange={(next) => {
              if (next !== selected)
                onNavigate({ kind: next === 'channels' ? 'event_gateway' : 'bots' });
            }}
          />
        </div>
      </div>
      {selected === 'channels' ? (
        channels
      ) : client.botViews ? (
        <GatewayConsumers
          key={state.status === 'authenticated' ? state.user.id : ''}
          client={client.botViews}
          selected={consumerRoute?.kind === 'bot' ? consumerRoute.botId : undefined}
          onNavigate={onNavigate}
        />
      ) : null}
    </nav>
  );
  const frame = (content: ReactNode, channels?: ReactNode, createAction?: SidebarNewAgent) => (
    <PortalFrame
      fullWidth
      route={{ kind: 'event_gateway' }}
      onNavigate={onNavigate}
      navigation={navigation('channels', channels)}
      createAction={createAction}
      sidebarCollapsed={sidebarCollapsed}
      onSidebarCollapsedChange={onSidebarCollapsedChange}
    >
      {content}
    </PortalFrame>
  );
  if (explicitlySignedOut) {
    return (
      <ServiceBody visible={visible}>
        {frame(
          state.status === 'error' ? (
            <IdentityError state={state} />
          ) : (
            <SignInLanding
              legalTerms={state.status === 'anonymous' ? state.legalTerms : undefined}
              onSignIn={() => signIn(routePath(route))}
            />
          ),
        )}
      </ServiceBody>
    );
  }
  return (
    <>
      {visitedChannels ? (
        <ServiceBody visible={visible && !consumers}>
          <ServicePage
            service="event-gateway"
            viewer={
              state.status === 'authenticated'
                ? `user:${state.user.id}`
                : state.status === 'anonymous'
                  ? 'anonymous'
                  : undefined
            }
            renderStatus={(content) => frame(content)}
          >
            {(session, denied, notice) => (
              <EventGateway
                session={session}
                denied={denied}
                active={visible && !consumers}
                onNavigate={() => {
                  if (window.innerWidth < SIDEBAR_AUTO_COLLAPSE) onSidebarCollapsedChange(true);
                }}
              >
                {({ sidebar, content, createAction }) =>
                  frame(
                    <>
                      {notice}
                      {content}
                    </>,
                    sidebar,
                    createAction,
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
            createAction={
              canManage
                ? {
                    label: 'New consumer',
                    icon: <IconSparkle16 />,
                    onCreate: () => onNavigate({ kind: 'bot_new' }),
                  }
                : undefined
            }
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
        legalTerms={state.legalTerms}
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
