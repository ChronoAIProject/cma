import { Console } from './Console';
import { useAuth } from './auth/context';
import lockup from './assets/brand/cma-lockup-horizontal.svg';
import { useAppRouter } from './routing/routes';
import { Button } from './ui/Button';
import './styles/tokens.css';
import './styles/design-platform.css';
import './styles/effects-typography.css';
import './styles/scrollbar.css';
import './styles/shiki.css';
import './styles/base.css';
import './styles/components.css';
import './AppError.css';
import { lazy, Suspense, useState } from 'react';
import { useCmaClient, useCreationClient, usePublicClient } from './api/context';
import type { CreationRequest } from './create/CreationFlow';

const CreationFlow = lazy(() =>
  import('./create/CreationFlow').then((module) => ({ default: module.CreationFlow })),
);
const PublicJourney = lazy(() =>
  import('./public/PublicJourney').then((module) => ({ default: module.PublicJourney })),
);

export function App() {
  const { state, signIn, signOut } = useAuth();
  const router = useAppRouter();
  const client = useCmaClient();
  const creation = useCreationClient();
  const publicClient = usePublicClient();
  const [request, setRequest] = useState<{
    readonly owner: string;
    readonly value: CreationRequest;
  }>();
  const [hierarchyRevision, setHierarchyRevision] = useState(0);

  if (
    (router.route.kind === 'public_trigger' || router.route.kind === 'launch') &&
    publicClient !== undefined &&
    state.status !== 'error'
  ) {
    return (
      <Suspense fallback={<div className="app-checking" aria-busy="true" />}>
        <PublicJourney client={publicClient} route={router.route} onNavigate={router.navigate} />
      </Suspense>
    );
  }

  if (state.status === 'checking') {
    return <div className="app-checking" aria-busy="true" />;
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
      <main className="auth-gate">
        <img className="auth-gate__brand" src={lockup} alt="CMA" />
        <Button variant="primary" onClick={() => void signIn(window.location.pathname)}>
          Sign in
        </Button>
      </main>
    );
  }
  return (
    <>
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
        onUseProfile={
          creation === undefined
            ? undefined
            : (selection) =>
                setRequest({ owner: state.user.id, value: { kind: 'profile_use', selection } })
        }
        onSignOut={() => {
          setRequest(undefined);
          void signOut();
        }}
      />
      {request === undefined ||
      request.owner !== state.user.id ||
      creation === undefined ||
      state.user.visitor ||
      !state.user.capabilities.includes('use_own_sandboxes') ? null : (
        <Suspense fallback={null}>
          <CreationFlow
            key={state.user.id}
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
    </>
  );
}
