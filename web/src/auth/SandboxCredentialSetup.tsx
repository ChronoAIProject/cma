import { useEffect, useRef, useState } from 'react';
import { createClient } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { useAuth } from './context';
import './SandboxCredentialSetup.css';

export function SandboxCredentialSetup({
  returnTo = '/',
  expectedSubject,
  onChecked,
}: {
  readonly returnTo?: string;
  readonly expectedSubject?: string;
  readonly onChecked: (subject: string) => void;
}) {
  const { state } = useAuth();
  const [owner] = useState(
    expectedSubject ?? (state.status === 'authenticated' ? state.user.id : undefined),
  );
  const [auth] = useState(() => createClient().auth);
  const [opened, setOpened] = useState(false);
  const [blocked, setBlocked] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const popup = useRef<Window | null>(null);
  const request = useRef<AbortController | undefined>(undefined);
  const login = auth.loginUrl({ returnTo });
  useEffect(
    () => () => {
      request.current?.abort();
      popup.current?.close();
    },
    [],
  );
  const open = () => {
    popup.current?.close();
    popup.current = window.open('about:blank', '_blank', 'popup,width=520,height=720');
    setError(undefined);
    setBlocked(popup.current === null);
    if (popup.current !== null) {
      popup.current.opener = null;
      popup.current.location.href = login;
      setOpened(true);
    }
  };
  const check = async () => {
    if (request.current !== undefined || !opened) return;
    const controller = new AbortController();
    request.current = controller;
    setBusy(true);
    setError(undefined);
    try {
      const session = await auth.readSession({ signal: controller.signal });
      if (controller.signal.aborted) return;
      if (!session.authenticated || owner === undefined || session.subject !== owner) {
        setError('Sign in with the same CMA account that made this request.');
        return;
      }
      // Identity matches; only the subsequent command can prove credential readiness.
      onChecked(owner);
    } catch {
      if (!controller.signal.aborted) setError('Your CMA account could not be checked. Try again.');
    } finally {
      request.current = undefined;
      if (!controller.signal.aborted) setBusy(false);
    }
  };
  return (
    <section className="sandbox-credential-setup" aria-label="CMA sign-in required">
      <Button onClick={open} disabled={busy}>
        Sign in to CMA
      </Button>
      {blocked ? (
        <>
          <p>The sign-in window was blocked.</p>
          <a href={login} target="_blank" rel="noopener noreferrer" onClick={() => setOpened(true)}>
            Open CMA sign-in
          </a>
        </>
      ) : null}
      {opened ? (
        <Button onClick={() => void check()} disabled={busy}>
          {busy ? 'Checking account' : 'Check signed-in account'}
        </Button>
      ) : null}
      {error === undefined ? null : <p role="alert">{error}</p>}
    </section>
  );
}
