import { Console } from './Console';
import { useAuth } from './auth/context';
import { BrandLoading } from './landing/LandingCharacter';
import { ShellLoading } from './ui/Skeleton';
import { SignInLanding } from './landing/SignInLanding';
import { parseRoute, useAppRouter } from './routing/routes';
import './styles/tokens.css';
import './styles/design-platform.css';
import './styles/effects-typography.css';
import './styles/scrollbar.css';
import './styles/shiki.css';
import './styles/base.css';
import './styles/components.css';
import './AppError.css';
import { lazy, Suspense, useLayoutEffect, useState } from 'react';
import { Presence } from './ui/Presence';
import { Button } from './ui/Button';
import { useCmaClient, useCreationClient, usePublicClient } from './api/context';
import type { CreationRequest } from './create/CreationFlow';
import { ReferenceProvider } from './profiles/ReferenceLink';
import { retainFatherDraftForSignIn } from './bots/fatherDraft';

const CreationFlow = lazy(() =>
  import('./create/CreationFlow').then((module) => ({ default: module.CreationFlow })),
);
const PublicJourney = lazy(() =>
  import('./public/PublicJourney').then((module) => ({ default: module.PublicJourney })),
);

import type { AppRouter, AppRoute } from './routing/routes';
import { EventGatewayPortal } from './services/EventGatewayPortal';
import { isBotRoute } from './BotManagement';
import { ServiceBody } from './services/ServiceBody';
import { PortalFrame } from './shell/PortalFrame';

const AUTHENTICATED_HOME_ROUTE: AppRoute = { kind: 'agent_manager' };

export function App() {
  const browserRouter = useAppRouter();
  const { state: identity } = useAuth();
  const { navigate } = browserRouter;
  const redirectHome = identity.status === 'authenticated' && browserRouter.route.kind === 'home';
  useLayoutEffect(() => {
    if (redirectHome) navigate(AUTHENTICATED_HOME_ROUTE, { replace: true });
  }, [redirectHome, navigate]);
  // Render the destination immediately, before the home URL is replaced, so
  // resolving identity never paints the anonymous landing for a signed-in owner.
  const router: AppRouter = redirectHome
    ? { ...browserRouter, route: AUTHENTICATED_HOME_ROUTE }
    : browserRouter;
  const identityKey = identity.status === 'authenticated' ? identity.user.id : identity.status;
  const service =
    router.route.kind === 'event_gateway' || isBotRoute(router.route) ? 'event-gateway' : undefined;
  const [lastGatewayRoute, setLastGatewayRoute] = useState<AppRoute>(
    service ? router.route : { kind: 'event_gateway' },
  );
  if (service && lastGatewayRoute !== router.route) setLastGatewayRoute(router.route);
  const [visitedGateway, setVisitedGateway] = useState(service !== undefined);
  if (service && !visitedGateway) setVisitedGateway(true);
  const home = router.route.kind === 'home';
  const [sidebarState, setSidebarState] = useState({ home, collapsed: home });
  // Reset only when crossing the home boundary, including browser history.
  // Navigation within a service or between services retains the manual choice.
  if (sidebarState.home !== home) setSidebarState({ home, collapsed: home });
  const sidebar = {
    sidebarCollapsed: sidebarState.collapsed,
    onSidebarCollapsedChange: (collapsed: boolean) => setSidebarState({ home, collapsed }),
  };
  const agentVisible = service === undefined && !home;
  const [lastAgentRoute, setLastAgentRoute] = useState<AppRoute>(
    agentVisible ? router.route : { kind: 'agent_manager' },
  );
  const [visitedAgent, setVisitedAgent] = useState(agentVisible);
  if (agentVisible && lastAgentRoute !== router.route) {
    setLastAgentRoute(router.route);
    setVisitedAgent(true);
  }
  const publicJourney = router.route.kind === 'public_trigger' || router.route.kind === 'launch';
  if (publicJourney) return <AgentApp router={router} />;
  const navigateService = (next: AppRoute) =>
    router.navigate(next.kind === 'agent_manager' ? lastAgentRoute : next);
  return (
    <div className="service-shell">
      {home ? (
        <ServiceBody>
          <PortalFrame route={router.route} onNavigate={navigateService} {...sidebar}>
            <HomeLanding router={router} />
          </PortalFrame>
        </ServiceBody>
      ) : null}
      {agentVisible || visitedAgent ? (
        <ServiceBody visible={agentVisible}>
          <AgentApp
            router={{ ...router, route: agentVisible ? router.route : lastAgentRoute }}
            onServiceNavigate={navigateService}
            {...sidebar}
          />
        </ServiceBody>
      ) : null}
      {visitedGateway ? (
        <EventGatewayPortal
          key={`event-gateway:${identityKey}`}
          visible={service === 'event-gateway'}
          route={service ? router.route : lastGatewayRoute}
          onNavigate={navigateService}
          {...sidebar}
        />
      ) : null}
    </div>
  );
}
function HomeLanding({ router }: { readonly router: AppRouter }) {
  const { state, signIn } = useAuth();
  if (state.status === 'checking') return <BrandLoading />;
  return (
    <SignInLanding
      onSignIn={async (returnTo = '/agent-manager') => {
        if (state.status === 'authenticated') router.navigate(parseRoute(returnTo));
        else {
          retainFatherDraftForSignIn();
          await signIn(returnTo);
        }
      }}
    />
  );
}
function AgentApp({
  router,
  sidebarCollapsed,
  onSidebarCollapsedChange,
  onServiceNavigate,
}: {
  readonly router: AppRouter;
  readonly sidebarCollapsed?: boolean;
  readonly onSidebarCollapsedChange?: (collapsed: boolean) => void;
  readonly onServiceNavigate?: (route: AppRoute) => void;
}) {
  const { state, signIn, signOut } = useAuth();
  const client = useCmaClient();
  const creation = useCreationClient();
  const publicClient = usePublicClient();
  const [request, setRequest] = useState<{
    readonly owner: string;
    readonly value: CreationRequest;
  }>();
  const [hierarchyRevision, setHierarchyRevision] = useState(0);

  // NyxID hosted connection links redirect their popup here after the
  // credential is saved. The main CMA window is already polling the durable
  // connection state. Keep its child window available for the next service;
  // the main window closes it only after the entire sequence is verified.
  if (window.location.pathname === '/connection-return') {
    return <ConnectionReturnPage />;
  }

  if (
    (router.route.kind === 'public_trigger' || router.route.kind === 'launch') &&
    publicClient !== undefined &&
    state.status !== 'error'
  ) {
    return (
      <ReferenceProvider
        key={state.status === 'authenticated' ? state.user.id : 'anonymous'}
        onNavigate={router.navigate}
      >
        <Suspense fallback={<BrandLoading />}>
          <PublicJourney client={publicClient} route={router.route} onNavigate={router.navigate} />
        </Suspense>
      </ReferenceProvider>
    );
  }

  if (state.status === 'checking') {
    return <ShellLoading label="Signing you in" />;
  }
  if (state.status === 'error') {
    return (
      <main className="app-error" role="alert">
        <h1 className="app-error__title">CMA could not reach its service</h1>
        <p className="app-error__next">It may be starting up. Reload to try again.</p>
        <Button variant="primary" onClick={() => window.location.reload()}>
          Reload
        </Button>
        <p className="app-error__what">{state.message}</p>
      </main>
    );
  }
  if (state.status === 'anonymous') {
    return (
      <PortalFrame
        route={router.route}
        onNavigate={onServiceNavigate ?? router.navigate}
        sidebarCollapsed={sidebarCollapsed ?? false}
        onSidebarCollapsedChange={onSidebarCollapsedChange ?? (() => {})}
        landing
      >
        <SignInLanding
          onSignIn={async (returnTo) => {
            retainFatherDraftForSignIn();
            await signIn(
              returnTo ??
                (router.route.kind === 'home' ? '/agent-manager' : window.location.pathname),
            );
          }}
        />
      </PortalFrame>
    );
  }
  return (
    <ReferenceProvider key={state.user.id} onNavigate={router.navigate}>
      <Console
        key={state.user.id}
        user={state.user}
        route={router.route}
        onNavigate={router.navigate}
        hierarchyRevision={hierarchyRevision}
        sidebarCollapsed={sidebarCollapsed}
        onSidebarCollapsedChange={onSidebarCollapsedChange}
        onServiceNavigate={onServiceNavigate}
        onCreate={
          creation === undefined
            ? undefined
            : (value) => setRequest({ owner: state.user.id, value })
        }
        onSignOut={() => {
          setRequest(undefined);
          void signOut();
        }}
      />
      <Presence>
        {request === undefined ||
        request.owner !== state.user.id ||
        creation === undefined ||
        state.user.visitor ||
        !state.user.capabilities.includes('use_own_sandboxes') ? null : (
          <Suspense fallback={null}>
            <CreationFlow
              key={state.user.id}
              viewerId={state.user.id}
              request={request.value}
              client={client}
              catalog={creation}
              canCreateSandbox={state.user.capabilities.includes('create_sandbox')}
              onClose={() => setRequest(undefined)}
              onChanged={() => setHierarchyRevision((value) => value + 1)}
              onCreated={(route) => {
                setRequest(undefined);
                setHierarchyRevision((value) => value + 1);
                router.navigate(route);
              }}
            />
          </Suspense>
        )}
      </Presence>
    </ReferenceProvider>
  );
}

function ConnectionReturnPage() {
  return (
    <main className="app-error">
      <h1 className="app-error__title">Returning to CMA</h1>
      <p className="app-error__next">
        CMA is checking your connection. Your next sign-in will open here, or this window will close
        when all services are ready. If nothing happens, return to the CMA window to continue.
      </p>
      <button type="button" onClick={() => window.close()}>
        Close this window
      </button>
    </main>
  );
}
