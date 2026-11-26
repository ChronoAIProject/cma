import { Presence } from '../ui/Presence';
import { useCallback, useEffect, useRef, useState } from 'react';
import { LandingCharacter } from './LandingCharacter';
import { Button } from '../ui/Button';
import { LegalNotice } from '../auth/LegalNotice';
import type { LegalTerms } from '../auth/types';
import './SignInLanding.css';

/** Where each service card returns after sign-in; every path is a canonical CMA route. */
const SERVICES = [
  {
    name: 'Agent Manager',
    summary: 'A place to think, build and return to your work. Give every Agent its own workspace.',
    path: '/agent-manager',
  },
  {
    name: 'Event Gateway',
    summary: 'Bring your Agents into the flow of your work with connected event channels.',
    path: '/event-gateway',
  },
] as const;

const STEPS = [
  {
    title: 'Start with an idea',
    body: 'Open an Agent in a ready-to-use cloud environment. Add a GitHub repository if you have one.',
  },
  {
    title: 'Describe the work',
    body: 'Ask a question, share files, or describe what you want to build. Your Agent gets to work.',
  },
  {
    title: 'Steer, approve, come back',
    body: 'Guide the work as it happens. Your files and conversation stay together when you return.',
  },
] as const;

export function SignInLanding({
  onSignIn,
  legalTerms,
}: {
  readonly onSignIn: (returnTo?: string) => Promise<void>;
  /** Published terms the person accepts by continuing. */
  readonly legalTerms?: LegalTerms;
}) {
  const [entering, setEntering] = useState<{ readonly returnTo?: string }>();
  const [failed, setFailed] = useState(false);
  const pending = useRef(false);
  const signIn = useRef(onSignIn);
  useEffect(() => {
    signIn.current = onSignIn;
  }, [onSignIn]);
  const activate = useCallback((returnTo?: string) => {
    if (pending.current) return;
    pending.current = true;
    setFailed(false);
    setEntering({ returnTo });
  }, []);
  useEffect(() => {
    if (entering === undefined) return;
    const timer = setTimeout(
      () => {
        void signIn.current(entering.returnTo).catch(() => {
          pending.current = false;
          setEntering(undefined);
          setFailed(true);
        });
      },
      window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 200,
    );
    return () => clearTimeout(timer);
  }, [entering]);
  const busy = entering !== undefined;
  return (
    <main
      className={`auth-gate sign-in-landing${busy ? ' sign-in-landing--entering' : ''}`}
      aria-busy={busy}
    >
      <div className="sign-in-landing__layout">
        <section className="sign-in-landing__hero" aria-labelledby="sign-in-landing-title">
          <div className="sign-in-landing__copy">
            <h1 id="sign-in-landing-title" className="sign-in-landing__eyebrow">
              ChronoAI Managed Agent
            </h1>
            <p className="sign-in-landing__headline">
              A little direction.
              <br />A world of possibility.
            </p>
            <p className="sign-in-landing__lede">
              Your ideas deserve room to grow. Work with a coding Agent in its own private cloud
              environment, with your files, tools and conversation in one place. Nothing to install.
            </p>
            <div className="sign-in-landing__actions">
              <Button variant="primary" onClick={() => activate()} disabled={busy}>
                Get Started
              </Button>
              <a className="button button--outline button--md" href="#sign-in-landing-how">
                See how it works
              </a>
            </div>
            <p className="sign-in-landing__note" role="status">
              {busy ? 'Opening NyxID sign-in…' : 'Sign in with NyxID. You’ll come straight back.'}
            </p>
            {legalTerms === undefined ? null : (
              <LegalNotice terms={legalTerms} className="sign-in-landing__note" />
            )}
            <Presence>
              {failed ? (
                <div className="sign-in-landing__alert" role="alert">
                  <p>Sign-in could not be started.</p>
                  <Button variant="outline" size="sm" onClick={() => activate()}>
                    Try again
                  </Button>
                </div>
              ) : null}
            </Presence>
          </div>
          <div className="brand-home sign-in-landing__brand">
            <LandingCharacter key={String(failed)} />
          </div>
        </section>

        <section
          id="sign-in-landing-how"
          className="sign-in-landing__section"
          aria-labelledby="sign-in-landing-how-title"
        >
          <p className="sign-in-landing__kicker">How it works</p>
          <h2 id="sign-in-landing-how-title">From a first thought to work in motion</h2>
          <ol className="sign-in-landing__grid">
            {STEPS.map((step, index) => (
              <li key={step.title} className="sign-in-landing__card">
                <span className="sign-in-landing__step" aria-hidden="true">
                  {String(index + 1).padStart(2, '0')}
                </span>
                <h3>{step.title}</h3>
                <p>{step.body}</p>
              </li>
            ))}
          </ol>
        </section>

        <section
          className="sign-in-landing__section"
          aria-labelledby="sign-in-landing-services-title"
        >
          <p className="sign-in-landing__kicker">Services</p>
          <h2 id="sign-in-landing-services-title">Your work, connected.</h2>
          <div className="sign-in-landing__grid">
            {SERVICES.map((service) => (
              <article key={service.name} className="sign-in-landing__card">
                <h3>{service.name}</h3>
                <p>{service.summary}</p>
                <Button
                  variant="outline"
                  size="sm"
                  placement="sign-in-landing__open"
                  onClick={() => activate(service.path)}
                  disabled={busy}
                >
                  Open {service.name}
                </Button>
              </article>
            ))}
          </div>
        </section>
      </div>
    </main>
  );
}
