import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  ApiError,
  createSession,
  fetchSession,
  listKernels,
  listSessions,
  resolveConfig,
  terminateSession,
  type SessionView,
} from './api';
import { TerminalView } from './terminal/TerminalView';
import './app.css';

/** Recovery states that are routine rather than failures. */
const ROUTINE_RECOVERY = new Set(['normal', 'idle', 'recovering', 'unknown']);

export default function App() {
  const config = useMemo(() => resolveConfig(), []);
  const [authenticated, setAuthenticated] = useState<boolean | null>(null);
  const [subject, setSubject] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionView[]>([]);
  const [kernels, setKernels] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [status, setStatus] = useState('idle');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await listSessions(config.apiBaseUrl);
      setSessions(list);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [config.apiBaseUrl]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const s = await fetchSession(config.apiBaseUrl);
        if (cancelled) return;
        setAuthenticated(s.authenticated);
        setSubject(s.subject ?? null);
        if (s.authenticated) {
          await refresh();
          const k = await listKernels(config.apiBaseUrl);
          if (!cancelled) setKernels(k.kernels);
        }
      } catch (e) {
        if (!cancelled) {
          setAuthenticated(false);
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [config.apiBaseUrl, refresh]);

  const onCreate = async () => {
    setBusy(true);
    setError(null);
    try {
      const kernel = kernels[0] ?? 'codex';
      const created = await createSession(config.apiBaseUrl, kernel, 120, 32);
      await refresh();
      setActive(created.id);
    } catch (e) {
      setError(
        e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e),
      );
    } finally {
      setBusy(false);
    }
  };

  const onTerminate = async (id: string) => {
    try {
      await terminateSession(config.apiBaseUrl, id);
      if (active === id) setActive(null);
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  if (authenticated === null) {
    return (
      <main className="shell">
        <div className="skeleton" aria-busy="true" aria-label="Loading" />
      </main>
    );
  }

  if (!authenticated) {
    return (
      <main className="shell centered">
        <div className="card">
          <h1>chrono-bot</h1>
          <p className="muted">Sign in to create and attach to agent sessions.</p>
          <a className="button primary" href={`${config.apiBaseUrl}/auth/login`}>
            Sign in
          </a>
          {error && <p className="error">{error}</p>}
        </div>
      </main>
    );
  }

  return (
    <main className="shell">
      <header className="bar">
        <div className="bar-left">
          <strong>chrono-bot</strong>
          <span className="muted">terminal</span>
        </div>
        <div className="bar-right">
          {subject && <span className="muted">{subject}</span>}
          <a className="button" href={`${config.apiBaseUrl}/auth/logout`}>
            Sign out
          </a>
        </div>
      </header>

      <div className="layout">
        <aside className="sidebar">
          <button className="button primary full" onClick={onCreate} disabled={busy}>
            {busy ? 'Creating…' : 'New session'}
          </button>
          <ul className="sessions">
            {sessions.map((s) => {
              const routine = ROUTINE_RECOVERY.has(s.recovery_state);
              return (
                <li key={s.id} className={s.id === active ? 'session active' : 'session'}>
                  <button className="session-open" onClick={() => setActive(s.id)}>
                    <span className="mono">{s.id.slice(0, 16)}</span>
                    <span className={routine ? 'chip' : 'chip danger'}>{s.status}</span>
                  </button>
                  <button
                    className="session-kill"
                    onClick={() => onTerminate(s.id)}
                    aria-label={`Terminate ${s.id}`}
                  >
                    ×
                  </button>
                </li>
              );
            })}
            {sessions.length === 0 && <li className="muted empty">No sessions yet.</li>}
          </ul>
        </aside>

        <section className="content">
          {active ? (
            <>
              <div className="content-bar">
                <span className="mono">{active}</span>
                <span className="muted">{status}</span>
              </div>
              <TerminalView
                apiBaseUrl={config.apiBaseUrl}
                sessionId={active}
                onStatus={setStatus}
              />
            </>
          ) : (
            <div className="placeholder muted">
              Select a session, or create one to open a terminal.
            </div>
          )}
        </section>
      </div>

      {error && <div className="toast error">{error}</div>}
    </main>
  );
}
