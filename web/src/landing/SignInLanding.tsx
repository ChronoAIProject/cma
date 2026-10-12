import { Presence } from '../ui/Presence';
import { useCallback, useEffect, useRef, useState } from 'react';
import { LandingCharacter } from './LandingCharacter';
import { Button } from '../ui/Button';
import './SignInLanding.css';

export function SignInLanding({ onSignIn }: { readonly onSignIn: () => Promise<void> }) {
  const [entering, setEntering] = useState(false);
  const [failed, setFailed] = useState(false);
  const pending = useRef(false);
  const signIn = useRef(onSignIn);
  useEffect(() => {
    signIn.current = onSignIn;
  }, [onSignIn]);
  const activate = useCallback(() => {
    if (pending.current) return;
    pending.current = true;
    setEntering(true);
  }, []);
  useEffect(() => {
    if (!entering) return;
    const timer = setTimeout(
      () => {
        void signIn.current().catch(() => {
          pending.current = false;
          setEntering(false);
          setFailed(true);
        });
      },
      window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 200,
    );
    return () => clearTimeout(timer);
  }, [entering]);
  return (
    <main
      className={`auth-gate sign-in-landing${entering ? ' sign-in-landing--entering' : ''}`}
      aria-busy={entering}
    >
      <div className="sign-in-landing__layout">
        <div className="brand-home sign-in-landing__brand">
          <LandingCharacter key={String(failed)} onActivate={activate} />
          <h1>ChronoAI Managed Agent</h1>
        </div>
        <aside className="sign-in-landing__services" aria-label="CMA services">
          <p className="sign-in-landing__eyebrow">One console, three services</p>
          <article>
            <h2>Agent Manager</h2>
            <p>Create and operate persistent agents with their own workspaces.</p>
          </article>
          <article>
            <h2>Image Manager</h2>
            <p>Choose a trusted public or private runtime image for each agent.</p>
          </article>
          <article>
            <h2>Event Gateway</h2>
            <p>Connect event channels and deliver verified events to your agents.</p>
          </article>
        </aside>
        <Presence>
          {failed ? (
            <div role="alert">
              <p>Sign-in could not be started.</p>
              <Button onClick={activate}>Try again</Button>
            </div>
          ) : null}
        </Presence>
      </div>
    </main>
  );
}
