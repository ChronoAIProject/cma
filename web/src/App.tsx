import { Suspense, lazy } from 'react';
import { IntroTerminal } from './landing/IntroTerminal';
import { TopBar } from './shell/TopBar';
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
 * One page, two states.
 *
 * Unauthenticated it is a terminal that introduces itself and takes a prompt;
 * authenticated it is the same surface driven by live sessions. There is no
 * navigation and no second route — see CLAUDE.md and docs/Web_Style.md.
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
        // The same bar as the signed-in state: a visitor and an owner are
        // looking at one product, not two.
        <div className="app-shell">
          <TopBar />
          <IntroTerminal />
        </div>
      );

    default: {
      const exhaustive: never = state;
      return exhaustive;
    }
  }
}
