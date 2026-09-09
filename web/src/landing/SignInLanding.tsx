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
      <div className="brand-home">
        <LandingCharacter key={String(failed)} onActivate={activate} />
        <h1>ChronoAI Managed Agent</h1>
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
