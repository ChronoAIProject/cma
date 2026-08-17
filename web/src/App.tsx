import { Suspense, lazy } from 'react';
import { IntroTerminal } from './landing/IntroTerminal';
import { PageFrame } from './shell/PageFrame';
import { useAuth } from './auth/context';
import './styles/tokens.css';
import './styles/base.css';
import './styles/components.css';

/*
 * The console pulls in xterm.js, roughly 400KB before compression. A visitor
 * who never signs in should never download it, so it is split out and fetched
 * only once there is an identity. The landing page is the first impression and
 * is frequently the whole visit.
 */
const Console = lazy(async () => {
  const module = await import('./Console');
  return { default: module.Console };
});

/**
 * One page frame, two states.
 *
 * Unauthenticated it is a terminal that introduces itself and takes a prompt;
 * authenticated it is the same frame carrying the two views docs/Web_Style.md
 * §1 admits — sessions and operations.
 *
 * There is still no router and no second route. The app bar's switch chooses
 * between views and nothing is pushed onto history, which is what keeps the
 * frame from becoming navigation. A view is where somebody is right now, so it
 * does not persist; an arrangement is a setting about how a list is drawn, and
 * that does.
 */
export function App() {
  const { state, signOut } = useAuth();

  switch (state.status) {
    case 'checking':
      // Deliberately empty rather than a spinner: the check resolves against a
      // cookie in a few milliseconds, and a flash of loading chrome is worse
      // than a frame of nothing.
      return <div className="app-checking" aria-busy="true" />;

    case 'authenticated':
      return (
        // Web_Style.md forbids a full-page spinner; the console arrives from
        // cache or a same-origin chunk, so an empty frame is the honest wait.
        <Suspense fallback={<div className="app-checking" aria-busy="true" />}>
          <Console user={state.user} onSignOut={() => void signOut()} />
        </Suspense>
      );

    case 'error':
      return (
        <main className="app-error" role="alert">
          <p>{state.message}</p>
          <p>Reload the page to try again.</p>
        </main>
      );

    case 'anonymous':
      return (
        // The same frame as the signed-in state: a visitor and an owner are
        // looking at one product, not two. No view switch, because a visitor
        // has no views to move between — the switch is not navigation and is
        // not drawn where there is nothing to select.
        <PageFrame showing="Signed out">
          <IntroTerminal />
        </PageFrame>
      );

    default: {
      const exhaustive: never = state;
      return exhaustive;
    }
  }
}
