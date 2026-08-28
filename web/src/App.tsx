import { Suspense, lazy } from 'react';
import { Landing } from './landing/Landing';
import { useAuth } from './auth/context';
/*
 * Mount order matters — `docs/Web_Style.md` §2. The palette sheets must be
 * parsed before anything reads an alias off them, and `tokens.css` first
 * because the others reference the font stacks and the easing curve it
 * declares.
 */
import './styles/tokens.css';
import './styles/design-platform.css';
import './styles/effects-typography.css';
import './styles/scrollbar.css';
import './styles/shiki.css';
import './styles/base.css';
import './styles/components.css';
import './AppError.css';

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
 * authenticated it is the same frame carrying the three views docs/Web_Style.md
 * §1 admits — sessions, operations and Developer.
 *
 * There is still no router and no second route. Account-menu actions choose
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
          <p className="app-error__what">{state.message}</p>
          <p className="app-error__next">Reload the page to try again.</p>
        </main>
      );

    case 'anonymous':
      return (
        /*
         * No frame at all. §9's frame is three columns of session, and a
         * visitor has no sessions — wrapping the landing in an empty sidebar
         * and an empty details column would draw two panels that say nothing
         * and cannot be opened.
         */
        <Landing />
      );

    default: {
      const exhaustive: never = state;
      return exhaustive;
    }
  }
}
