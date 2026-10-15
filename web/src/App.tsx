import { Console } from './Console';
import { useAuth } from './auth/context';
import { BrandLoading } from './landing/LandingCharacter';
import { SignInLanding } from './landing/SignInLanding';
import { useAppRouter } from './routing/routes';
import './styles/tokens.css';
import './styles/design-platform.css';
import './styles/effects-typography.css';
import './styles/scrollbar.css';
import './styles/shiki.css';
import './styles/base.css';
import './styles/components.css';
import './AppError.css';
import { lazy, Suspense, useState } from 'react';
import { Presence } from './ui/Presence';
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
import { ServiceTabs } from './services/ServiceTabs';
import { ServicePage } from './services/ServicePage';
import { ImageManager } from './services/ImageManager';
import { EventGateway } from './services/EventGateway';

export function App() {
  const router = useAppRouter();
  const service =
    router.route.kind === 'image_manager'
      ? 'image-manager'
      : router.route.kind === 'event_gateway'
        ? 'event-gateway'
        : undefined;
  const [visitedServices, setVisitedServices] = useState<
    readonly ('event-gateway' | 'image-manager')[]
  >(service ? [service] : []);
  if (service && !visitedServices.includes(service))
    setVisitedServices([...visitedServices, service]);
  const home = router.route.kind === 'home';
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
  return (
    <div className="service-shell">
      <ServiceTabs route={router.route} onNavigate={router.navigate} />
      {home ? (
        <div className="service-shell__body">
          <HomeLanding router={router} />
        </div>
      ) : null}
      {agentVisible || visitedAgent ? (
        <div className="service-shell__body" hidden={!agentVisible}>
          <AgentApp router={{ ...router, route: agentVisible ? router.route : lastAgentRoute }} />
        </div>
      ) : null}
      {visitedServices.map((visited) => (
        <div className="service-shell__body" key={visited} hidden={service !== visited}>
          <ServicePage service={visited}>
            {(session, denied) =>
              visited === 'image-manager' ? (
                <ImageManager session={session} denied={denied} />
              ) : (
                <EventGateway session={session} denied={denied} />
              )
            }
          </ServicePage>
        </div>
      ))}
    </div>
  );
}
function HomeLanding({ router }: { readonly router: AppRouter }) {
  const { state, signIn } = useAuth();
  if (state.status === 'checking') return <BrandLoading />;
  return (
    <SignInLanding
      onSignIn={async () => {
        if (state.status === 'authenticated') router.navigate({ kind: 'agent_manager' });
        else {
          retainFatherDraftForSignIn();
          await signIn('/agent-manager');
        }
      }}
    />
  );
}
function AgentApp({ router }: { readonly router: AppRouter }) {
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
    return <BrandLoading label="Signing you in" />;
  }
  if (state.status === 'error') {
    return (
      <main className="app-error" role="alert">
        <p className="app-error__what">{state.message}</p>
        <p className="app-error__next">Reload the page to try again.</p>
      </main>
    );
  }
  if (state.status === 'anonymous') {
    return (
      <SignInLanding
        onSignIn={async () => {
          retainFatherDraftForSignIn();
          await signIn(router.route.kind === 'home' ? '/agent-manager' : window.location.pathname);
        }}
      />
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
      <p className="app-error__what">Returning to CMA</p>
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
