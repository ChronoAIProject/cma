import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { capturePendingTurn } from './auth/pendingTurn';
import type { AuthProvider } from './auth/types';

/**
 * The console talks to the control plane, so these tests stand one up.
 *
 * Stubbed at the network boundary rather than by injecting a fake client: the
 * point of most of these assertions is that the real request path produces the
 * right surface, and a swapped-out client would stop testing that.
 */
const sockets: FakeSocket[] = [];
const streams: FakeEventSource[] = [];

/**
 * A stand-in for the progress stream.
 *
 * A session now starts as `provisioning` and becomes usable later, so the
 * console learns it is ready from here rather than from the create response.
 */
class FakeEventSource {
  static readonly CLOSED = 2;
  readyState = 1;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(readonly url: string) {
    streams.push(this);
    // Announce readiness on the next tick, as a real runtime would.
    queueMicrotask(() => this.announce('ready'));
  }
  announce(status: string): void {
    this.onmessage?.({
      data: JSON.stringify({ seq: 2, event: { type: 'status_changed', status } }),
    } as MessageEvent);
  }
  close(): void {
    this.readyState = FakeEventSource.CLOSED;
  }
}

class FakeSocket {
  static readonly OPEN = 1;
  readyState = 1;
  binaryType = '';
  sent: unknown[] = [];
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;

  constructor() {
    sockets.push(this);
    // The relay announces itself before anything may be sent.
    queueMicrotask(() => {
      this.onmessage?.({
        data: JSON.stringify({ t: 'ready', cols: 80, rows: 24 }),
      } as MessageEvent);
    });
  }
  send(data: unknown): void {
    this.sent.push(data);
  }
  close(): void {}
}

/** Text of everything written to the first relay opened. */
function sentText(): string {
  return sockets
    .flatMap((s) => s.sent)
    .map((f) =>
      ArrayBuffer.isView(f) ? new TextDecoder().decode(f as ArrayBufferView) : String(f),
    )
    .join('');
}

let created = 0;

beforeEach(() => {
  sockets.length = 0;
  created = 0;
  streams.length = 0;
  vi.stubGlobal('WebSocket', FakeSocket);
  vi.stubGlobal('EventSource', FakeEventSource);
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const json = (status: number, body: unknown) =>
        ({ ok: status < 400, status, json: async () => body }) as Response;

      if (url.endsWith('/api/v1/sessions') && (init?.method ?? 'GET') === 'GET') {
        // A fresh owner with nothing running, so the console opens one.
        return json(200, { sessions: [] });
      }
      if (url.endsWith('/api/v1/sessions') && init?.method === 'POST') {
        created += 1;
        // Creation answers before a runtime exists; readiness arrives on the
        // event stream.
        return json(201, {
          id: `sesn_${created}`,
          kernel: 'codex',
          status: 'provisioning',
          created_at: new Date(0).toISOString(),
        });
      }
      return json(404, {});
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function renderApp(provider: AuthProvider) {
  const result = render(
    <AuthContextProvider provider={provider}>
      <App />
    </AuthContextProvider>,
  );
  // The identity check resolves in a microtask.
  await act(async () => {});
  return result;
}

/**
 * The console is code-split so the landing page does not carry xterm, which
 * means it arrives a dynamic import later than everything else.
 */
async function waitForConsole() {
  return waitFor(() => screen.getByRole('tablist', { name: /sessions/i }));
}

/**
 * Start a session the way a person does: open the settings and confirm.
 *
 * Creating one is now a deliberate act with choices attached, so the tests go
 * through the same door rather than around it.
 */
async function startSession(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: /new session/i }));
  await user.click(await screen.findByRole('button', { name: /start session/i }));
}

describe('App', () => {
  it('shows the introduction to a visitor who is not signed in', async () => {
    await renderApp(createMockAuthProvider());

    expect(screen.getByLabelText(/what would you like chronobot to build/i)).toBeInTheDocument();
  });

  it('shows the live console to a signed-in user', async () => {
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));

    expect(await waitForConsole()).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /account menu/i })).toBeInTheDocument();
  });

  it('has no navigation menu in either state', async () => {
    // The product is one page. This is the rule most likely to be eroded by a
    // later change, so it is asserted rather than merely documented.
    const { unmount } = await renderApp(createMockAuthProvider());
    expect(screen.queryByRole('navigation')).toBeNull();
    unmount();

    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    expect(screen.queryByRole('navigation')).toBeNull();
  });

  it('starts no session until one is asked for', async () => {
    // Signing in is not a request to do work. Provisioning a sandbox because
    // someone opened the page spends real resources on an intention nobody
    // expressed, so the console waits to be asked.
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    expect(screen.queryAllByRole('tab')).toHaveLength(0);
    expect(await screen.findByText(/start one from the left/i)).toBeInTheDocument();
  });

  it('says a session is starting rather than appearing to hang', async () => {
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await startSession(user);

    // The tab exists immediately even though no runtime does yet.
    expect(await screen.findByRole('tab')).toBeInTheDocument();
  });

  it('carries the visitor prompt into the first session', async () => {
    // The prompt is delivered to the session rather than painted locally: a
    // real terminal shows it because the agent echoes it back, and drawing it
    // in the browser would show text the session never received.
    capturePendingTurn('build me a web scraper');

    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await startSession(user);

    await waitFor(() => {
      expect(sentText()).toContain('build me a web scraper\n');
    });
  });

  it('signs out back to the introduction', async () => {
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('menuitem', { name: /sign out/i }));

    await waitFor(() => {
      expect(screen.getByLabelText(/what would you like chronobot to build/i)).toBeInTheDocument();
    });
  });

  it('explains an identity failure instead of rendering a blank page', async () => {
    await renderApp(createMockAuthProvider({ failCurrent: 'NyxID is unreachable.' }));

    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('NyxID is unreachable.');
    // Errors must say what to do about it, per Web_Style.md.
    expect(alert).toHaveTextContent(/reload/i);
  });

  it('surfaces a provider that throws rather than hanging on the check', async () => {
    const broken: AuthProvider = {
      current: () => Promise.reject(new Error('boom')),
      signIn: vi.fn(),
      signOut: vi.fn(),
    };

    await renderApp(broken);

    // The contract says implementations must not throw, but the provider is
    // injected — a misbehaving one must not leave the page permanently blank.
    expect(screen.getByRole('alert')).toHaveTextContent('boom');
  });

  it('adds a session from the + control', async () => {
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    const before = screen.queryAllByRole('tab').length;

    await startSession(user);

    await waitFor(() => {
      expect(screen.getAllByRole('tab')).toHaveLength(before + 1);
    });
  });
});
