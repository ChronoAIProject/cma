import { Presence } from '../ui/Presence';
import { useCallback, useEffect, useRef, useState } from 'react';
import { LandingCharacter } from './LandingCharacter';
import { Button } from '../ui/Button';
import './SignInLanding.css';

/** Where each service card returns after sign-in; every path is a canonical CMA route. */
const SERVICES = [
  {
    name: 'Agent Manager',
    summary: 'Create and run Agents, each with its own Worktree, Workspace and Sandbox.',
    path: '/agent-manager',
  },
  {
    name: 'Event Gateway',
    summary: 'Connect event channels and deliver verified events to your Agents.',
    path: '/event-gateway',
  },
] as const;

const STEPS = [
  {
    title: 'Choose an image and settings',
    body: 'Pick the runtime image, the model, how hard it thinks, and how much it may do without asking.',
  },
  {
    title: 'Describe the work',
    body: 'Point it at a public GitHub repository or start from an empty folder, then write the first message.',
  },
  {
    title: 'Steer, approve, come back',
    body: 'Approve what needs you, steer while it works, and pick up any conversation later on any device.',
  },
] as const;

export function SignInLanding({
  onSignIn,
}: {
  readonly onSignIn: (returnTo?: string) => Promise<void>;
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
              <span aria-hidden="true" />
              ChronoAI Managed Agent
            </h1>
            <p className="sign-in-landing__headline">
              Run coding agents in the cloud. Nothing to install.
            </p>
            <p className="sign-in-landing__lede">
              Every Agent works in its own git Worktree inside a private Sandbox. Pause it, close
              the tab, come back next week: the files and the whole conversation are where you left
              them.
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
          <h2 id="sign-in-landing-how-title">One Agent, one Worktree, one conversation</h2>
          <ol className="sign-in-landing__grid">
            {STEPS.map((step, index) => (
              <li key={step.title} className="sign-in-landing__card">
                <span className="sign-in-landing__step" aria-hidden="true">
                  {index + 1}
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
          <h2 id="sign-in-landing-services-title">
            One console for your Agents and what feeds them
          </h2>
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
            <article className="sign-in-landing__card sign-in-landing__card--soon">
              <h3>
                Image Manager <span className="sign-in-landing__soon">Coming soon</span>
              </h3>
              <p>Build runtime images and share them privately, with your team, or publicly.</p>
            </article>
          </div>
        </section>
      </div>
    </main>
  );
}
