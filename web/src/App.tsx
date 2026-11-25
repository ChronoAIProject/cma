import { ConnectionReturnPage } from './connections/ConnectionReturnPage';
import { Console } from './Console';
import { useAuth } from './auth/context';
import { BrandLoading } from './landing/LandingCharacter';
import { ShellLoading } from './ui/Skeleton';
import { SignInLanding } from './landing/SignInLanding';
import { legalTermsOf } from './auth/types';
import { LegalConsentDialog } from './auth/LegalConsentDialog';
import { parseRoute, useAppRouter } from './routing/routes';
import './styles/tokens.css';
import './styles/design-platform.css';
import './styles/effects-typography.css';
import './styles/scrollbar.css';
import './styles/shiki.css';
import './styles/base.css';
import './styles/components.css';
import './AppError.css';
import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Presence } from './ui/Presence';
import { IdentityNotice, IdentityError } from './auth/IdentityNotice';
import { useCmaClient, useCreationClient, usePublicClient } from './api/context';
import type { CreationRequest } from './create/CreationFlow';
import { ReferenceProvider } from './profiles/ReferenceLink';
import { retainFatherDraftForSignIn } from './bots/fatherDraft';

const UsageDialog = lazy(() =>
  import('./usage/UsageDialog').then((module) => ({ default: module.UsageDialog })),
);
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
import { useReportOwner } from './usage/metrics/reportOwner';
import { PortalFrame } from './shell/PortalFrame';

const AUTHENTICATED_HOME_ROUTE: AppRoute = { kind: 'agent_manager' };

export function App() {
  const browserRouter = useAppRouter();
  const { state: identity, signOut } = useAuth();
  const identityScope =
    identity.status === 'authenticated'
      ? JSON.stringify([identity.user.id, [...identity.user.capabilities].sort()])
      : identity.status;
  const lastIdentityScope = useRef(identityScope);
  const [authEpoch, setAuthEpoch] = useState(0);
  useEffect(() => {
    if (lastIdentityScope.current === identityScope) return;
    lastIdentityScope.current = identityScope;
    setAuthEpoch((value) => value + 1);
  }, [identityScope]);
  const usageOwner = useReportOwner(
    identity.status === 'authenticated' ? identity.user.id : undefined,
    identity.status === 'authenticated' ? identity.user.capabilities : [],
    authEpoch,
  );
  const { navigate } = browserRouter;
  const client = useCmaClient();
  const usageOpen = browserRouter.route.kind === 'usage';
  const [visitedUsage, setVisitedUsage] = useState(usageOpen);
  if (usageOpen && !visitedUsage) setVisitedUsage(true);
  const usageDialog =
    identity.status === 'authenticated' && (usageOpen || visitedUsage) ? (
      <Suspense fallback={null}>
        <UsageDialog
          key={identity.user.id}
          client={client.usageViews}
          principal={usageOwner}
          open={usageOpen}
          appearanceNotice={browserRouter.usageAppearanceNotice}
          onClose={() => browserRouter.closeUsage?.()}
        />
      </Suspense>
    ) : null;
  const legalTerms = identity.status === 'authenticated' ? identity.user.legalTerms : undefined;
  const legalConsent =
    identity.status === 'authenticated' &&
    legalTerms !== undefined &&
    client.legalConsent !== undefined ? (
      <LegalConsentDialog
        key={`${identity.user.id}:${legalTerms.version}`}
        terms={legalTerms}
        client={client.legalConsent}
        onSignOut={() => void signOut()}
      />
    ) : null;
  const redirectHome = identity.status === 'authenticated' && browserRouter.route.kind === 'home';
  useLayoutEffect(() => {
    if (redirectHome) navigate(AUTHENTICATED_HOME_ROUTE, { replace: true });
  }, [redirectHome, navigate]);
  // Render the destination immediately, before the home URL is replaced, so
  // resolving identity never paints the anonymous landing for a signed-in owner.
  const router: AppRouter = redirectHome
    ? { ...browserRouter, route: AUTHENTICATED_HOME_ROUTE }
    : usageOpen
      ? { ...browserRouter, route: browserRouter.backgroundRoute ?? AUTHENTICATED_HOME_ROUTE }
      : browserRouter;
  /*
   * Retained portal work belongs to one known identity: a signed-in owner or a
   * signed-out visitor. The portal is not mounted while that identity is being
   * established, and an outage that hides it does not replace the instance: the
   * first known identity adopts what was mounted. Only a change from one known
   * identity to another discards what the portal retained. Keying on the
   * unresolved state remounted the portal when sign-in resolved, discarding a
   * channel editor the owner had already opened.
   */
  const knownIdentity =
    identity.status === 'authenticated'
      ? `user:${identity.user.id}`
      : identity.status === 'anonymous'
        ? 'anonymous'
        : undefined;
  const [portalOwner, setPortalOwner] = useState({ identity: knownIdentity, generation: 0 });
  if (knownIdentity !== undefined && knownIdentity !== portalOwner.identity)
    setPortalOwner({
      identity: knownIdentity,
      generation:
        portalOwner.identity === undefined ? portalOwner.generation : portalOwner.generation + 1,
    });
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
  if (publicJourney)
    return (
      <div className="service-shell">
        <IdentityNotice />
        <AgentApp router={router} authEpoch={authEpoch} />
        {usageDialog}
        {legalConsent}
      </div>
    );
  const navigateService = (next: AppRoute) =>
    router.navigate(next.kind === 'agent_manager' ? lastAgentRoute : next);
  return (
    <div className="service-shell">
      <IdentityNotice />
      {usageDialog}
      {legalConsent}
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
            authEpoch={authEpoch}
            visible={agentVisible && !usageOpen}
            onServiceNavigate={navigateService}
            {...sidebar}
          />
        </ServiceBody>
      ) : null}
      {visitedGateway ? (
        identity.status === 'checking' ? (
          <ServiceBody visible={service === 'event-gateway'}>
            <ShellLoading label="Signing you in" />
          </ServiceBody>
        ) : (
          <EventGatewayPortal
            key={`event-gateway:${portalOwner.generation}`}
            visible={service === 'event-gateway'}
            route={service ? router.route : lastGatewayRoute}
            onNavigate={navigateService}
            {...sidebar}
          />
        )
      ) : null}
    </div>
  );
}
function HomeLanding({ router }: { readonly router: AppRouter }) {
  const { state, signIn } = useAuth();
  if (state.status === 'checking') return <BrandLoading />;
  return (
    <SignInLanding
      legalTerms={legalTermsOf(state)}
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
  authEpoch,
  visible = true,
  sidebarCollapsed,
  onSidebarCollapsedChange,
  onServiceNavigate,
}: {
  readonly router: AppRouter;
  readonly authEpoch: number;
  readonly visible?: boolean;
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

  // Callback parameters never prove readiness. The original target verifies
  // the durable connection after a popup or explicit same-tab return.
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
  if (state.status === 'error') return <IdentityError state={state} />;
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
          legalTerms={state.legalTerms}
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
        authEpoch={authEpoch}
        visible={visible}
        route={router.route}
        onNavigate={router.navigate}
        hierarchyRevision={hierarchyRevision}
        sidebarCollapsed={sidebarCollapsed}
        onSidebarCollapsedChange={onSidebarCollapsedChange}
        onServiceNavigate={onServiceNavigate}
        onCreate={
          creation === undefined
            ? undefined
            : (value) => {
                if (
                  value.kind === 'new_agent' &&
                  !('workspaceId' in value) &&
                  !('sandboxId' in value)
                ) {
                  setRequest(undefined);
                  router.navigate({ kind: 'home' });
                  requestAnimationFrame(() =>
                    document.querySelector<HTMLTextAreaElement>('.task-home textarea')?.focus(),
                  );
                } else setRequest({ owner: state.user.id, value });
              }
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
