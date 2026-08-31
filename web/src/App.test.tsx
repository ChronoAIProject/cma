import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import { App } from './App';
import { MAX_LIVE_NODES } from './canvas/layout';
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
const streams: FakeEventStream[] = [];

/**
 * A controllable fetch-backed progress stream.
 *
 * A session now starts as `provisioning` and becomes usable later, so the
 * console learns it is ready from here rather than from the create response.
 */
class FakeEventStream {
  /** What the next session reports reaching. Not every start succeeds. */
  static outcome = 'ready';
  /**
   * A sign-in the next session stops to ask for, if any.
   *
   * Set, the stream reports the request and the session stays provisioning —
   * which is what actually happens, because the kernel raises this from inside
   * the conversation open and waits there. Announcing an outcome as well would
   * model a session that signs itself in.
   */
  static signIn: { url: string; code?: string; message?: string } | null = null;
  readonly response: Response;
  #controller!: ReadableStreamDefaultController<Uint8Array>;
  #sequence = 1;
  #closed = false;

  constructor(
    readonly sessionId: string,
    signal?: AbortSignal | null,
  ) {
    const body = new ReadableStream<Uint8Array>({
      start: (controller) => {
        this.#controller = controller;
      },
      cancel: () => {
        this.#closed = true;
      },
    });
    this.response = new Response(body, {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    });
    streams.push(this);
    signal?.addEventListener('abort', () => this.close(signal.reason), { once: true });

    // Announce on the next tick, as a real runtime would.
    queueMicrotask(() => {
      if (FakeEventStream.signIn !== null) {
        this.send({ type: 'sign_in_required', ...FakeEventStream.signIn });
        return;
      }
      this.announce(FakeEventStream.outcome);
    });
  }

  announce(status: string): void {
    this.send({ type: 'status_changed', status });
  }

  /** One SSE frame, in the exact envelope the control plane serialises. */
  send(event: unknown): void {
    if (this.#closed) {
      return;
    }
    this.#sequence += 1;
    const data = JSON.stringify({ seq: this.#sequence, session_id: this.sessionId, event });
    this.#controller.enqueue(
      new TextEncoder().encode(`id: ${String(this.#sequence)}\ndata: ${data}\n\n`),
    );
  }

  close(reason: unknown = new Error('event stream closed')): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#controller.error(reason);
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

/**
 * Sessions the control plane already has, as the list endpoint reports them.
 *
 * Seeded per test rather than fixed, because the behaviours that only appear
 * with a fleet — the canvas attachment budget, one session's action reaching
 * another — cannot be reached from an owner with nothing running.
 */
let listing: Record<string, unknown>[] = [];
let allowanceAnswer: unknown = { live: 0, exempt: false };
let allowanceFails = false;
let allowanceReads = 0;
let reviveProblem: { readonly status: number; readonly body: unknown } | null = null;
let deleteResponse: Promise<Response> | null = null;
let deleteRequested = false;

/** `count` running sessions, distinguishable by start time as the rail shows them. */
function seedSessions(count: number): void {
  listing = Array.from({ length: count }, (_, i) => ({
    id: `sesn_${i}`,
    kernel: 'codex',
    status: 'running',
    created_at: new Date(Date.UTC(2026, 7, 14, 3, i)).toISOString(),
  }));
}

/** A canvas node by session id, the way the canvas's own tests find one. */
function canvasNode(id: string): HTMLElement {
  const node = document.querySelector(`[data-node-id="${id}"]`);
  if (node === null) {
    throw new Error(`no node for session ${id}`);
  }
  return node as HTMLElement;
}

/** An attached node holds a terminal; an unattached one holds a sentence saying so. */
function isAttached(id: string): boolean {
  return within(canvasNode(id)).queryByRole('group') !== null;
}

beforeEach(() => {
  window.history.replaceState(null, '', '/');
  sockets.length = 0;
  created = 0;
  streams.length = 0;
  listing = [];
  allowanceAnswer = { live: 0, exempt: false };
  allowanceFails = false;
  allowanceReads = 0;
  reviveProblem = null;
  deleteResponse = null;
  deleteRequested = false;
  FakeEventStream.outcome = 'ready';
  FakeEventStream.signIn = null;
  vi.stubGlobal('WebSocket', FakeSocket);
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), window.location.origin);
      const path = url.pathname;
      const method = init?.method ?? 'GET';
      const json = (status: number, body: unknown) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { 'content-type': 'application/json' },
        });
      const problem = (status: number, body: Record<string, unknown>) =>
        new Response(JSON.stringify({ status, ...body }), {
          status,
          headers: { 'content-type': 'application/problem+json' },
        });

      if (path === '/api/v1/environment-profiles' && method === 'GET') {
        return json(200, []);
      }
      if (path === '/api/v1/triggers' && method === 'GET') {
        return json(200, { items: [], next_cursor: null });
      }
      if (path === '/api/v1/trigger-revisions' && method === 'GET') {
        return json(200, { items: [], next_cursor: null });
      }
      if (path === '/api/v1/trigger-connections' && method === 'GET') {
        return json(200, { connections: [] });
      }
      if (path === '/api/v1/kernels' && method === 'GET') {
        return json(200, {
          kernels: [{ id: 'codex', modes: ['terminal', 'conversation'] }],
        });
      }
      if (path === '/api/v1/sessions/allowance') {
        allowanceReads += 1;
        if (allowanceFails) {
          throw new Error('allowance unavailable');
        }
        return json(200, allowanceAnswer);
      }
      if (path === '/api/v1/pricing/quote') {
        return json(200, { priced: false });
      }
      if (path === '/api/v1/sessions' && method === 'GET') {
        // Empty unless a test seeded a fleet, which is the fresh-owner case.
        return json(200, { sessions: listing });
      }
      if (method === 'DELETE') {
        deleteRequested = true;
        return deleteResponse ?? new Response(null, { status: 204 });
      }
      if (path === '/api/v1/sessions' && method === 'POST') {
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
      if (path.endsWith('/revive') && method === 'POST' && reviveProblem !== null) {
        return problem(reviveProblem.status, reviveProblem.body as Record<string, unknown>);
      }

      const events = /^\/api\/v1\/sessions\/([^/]+)\/events$/.exec(path);
      if (events?.[1] !== undefined && method === 'GET') {
        return new FakeEventStream(decodeURIComponent(events[1]), init?.signal).response;
      }
      if (path.endsWith('/transcript') && method === 'GET') {
        return new Response(new Uint8Array(), {
          status: 200,
          headers: { 'content-type': 'application/octet-stream' },
        });
      }

      const session = /^\/api\/v1\/sessions\/([^/]+)$/.exec(path);
      if (session?.[1] !== undefined && method === 'GET') {
        const id = decodeURIComponent(session[1]);
        const existing = listing.find((candidate) => candidate.id === id);
        if (existing !== undefined) {
          return json(200, existing);
        }
        const createdIndex = Number(id.replace(/^sesn_/, ''));
        if (Number.isSafeInteger(createdIndex) && createdIndex > 0 && createdIndex <= created) {
          return json(200, {
            id,
            kernel: 'codex',
            status: 'ready',
            created_at: new Date(0).toISOString(),
          });
        }
      }

      return problem(404, { detail: 'The requested resource was not found.', code: 'not_found' });
    }),
  );
});

afterEach(() => {
  // The arrangement and the terminal palette persist per browser, so a test
  // that chooses one would otherwise choose it for every test after it.
  window.localStorage.clear();
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

/** Choose an arrangement from the rail's view-options card. */
async function arrange(user: UserEvent, name: 'List' | 'Canvas') {
  await user.click(screen.getByRole('button', { name: 'View options' }));
  await user.click(await screen.findByRole('menuitemradio', { name }));
}

/**
 * The console is code-split so the landing page does not carry xterm, which
 * means it arrives a dynamic import later than everything else.
 *
 * The wait is bounded well above the default, because what is being waited on
 * is a whole chunk being transformed and evaluated while the rest of the suite
 * competes for the same cores. At the default second this failed roughly one
 * run in three — never because the console was absent, only because it had not
 * finished arriving. A longer bound still fails if it never comes.
 */
async function waitForConsole() {
  // The rail groups by state, so there is a list per group rather than one
  // named "Sessions". Active is the one that is always open.
  return waitFor(() => screen.getByRole('tablist', { name: 'Active' }), { timeout: 10_000 });
}

/**
 * Start a session the way a person does: open the settings and confirm.
 *
 * Creating one is now a deliberate act with choices attached, so the tests go
 * through the same door rather than around it.
 */
async function startSession(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: /new session/i }));
  await user.click(await screen.findByRole('button', { name: /^continue$/i }));
  await user.click(await screen.findByRole('button', { name: /start session/i }));
}

describe('App', () => {
  it('shows the introduction to a visitor who is not signed in', async () => {
    await renderApp(createMockAuthProvider());

    expect(screen.getByLabelText(/what would you like cma to build/i)).toBeInTheDocument();
  });

  it.each([
    '/launches/launch-one',
    '/launches/launch-one/recover',
    '/launches/launch-one/contribute',
    '/sessions/session-one',
  ])('preserves the owner deep link through sign-in from %s', async (pathname) => {
    window.history.replaceState(null, '', pathname);
    const provider = createMockAuthProvider();
    const signIn = vi.spyOn(provider, 'signIn').mockResolvedValue();
    await renderApp(provider);

    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    expect(signIn).toHaveBeenCalledWith(pathname);
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

  it('reads the allowance once while restoring and leaves creation enabled when it fails', async () => {
    allowanceFails = true;
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await waitFor(() => {
      expect(allowanceReads).toBe(1);
    });
    expect(screen.getByRole('button', { name: /new session/i })).toBeEnabled();
  });

  it('carries an at-limit allowance to the sidebar and command bar', async () => {
    allowanceAnswer = { limit: 3, live: 3, exempt: false };
    const reason = 'You have 3 of 3 sessions running. Pause or end one to start another.';
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    const newSession = screen.getByRole('button', { name: /new session/i });
    await waitFor(() => {
      expect(newSession).toBeDisabled();
    });
    expect(newSession).toHaveAccessibleDescription(reason);

    await user.click(screen.getByRole('button', { name: /find a session/i }));
    const row = screen
      .getAllByRole('option')
      .find((option) => option.textContent?.includes('New session'));
    expect(row).toHaveTextContent(reason);
    const command = within(row as HTMLElement).getByRole('button');
    expect(command).toBeDisabled();
  });

  it('renders a revive limit refusal verbatim and keeps the session paused', async () => {
    listing = [
      {
        id: 'sesn_paused',
        kernel: 'codex',
        status: 'idle',
        created_at: new Date(0).toISOString(),
      },
    ];
    allowanceAnswer = { limit: 3, live: 2, exempt: false };
    const detail =
      '4 sessions are already running for this owner and the limit is 3; nothing new starts until fewer than 3 are running';
    reviveProblem = {
      status: 409,
      body: { detail, code: 'session_limit_reached', limit: 3, live: 4 },
    };
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^resume$/i }));

    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText(detail)).toBeInTheDocument();
    expect(alert).toHaveTextContent('session_limit_reached');
    expect(alert).toHaveTextContent('The session stays stopped.');
    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    expect(screen.getByRole('menuitem', { name: /^resume$/i })).toBeInTheDocument();
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
      expect(screen.getByLabelText(/what would you like cma to build/i)).toBeInTheDocument();
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

  it('opens and closes the command bar with the keyboard', async () => {
    // §6: ⌘K opens the plate and closes it again; the trigger in the bar is
    // a real control and never a dead field.
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    expect(
      screen.getByRole('button', { name: /find a session, or type a command/i }),
    ).toBeEnabled();

    await user.keyboard('{Meta>}k{/Meta}');
    expect(await screen.findByRole('dialog', { name: /commands/i })).toBeInTheDocument();

    await user.keyboard('{Meta>}k{/Meta}');
    expect(screen.queryByRole('dialog', { name: /commands/i })).toBeNull();
  });

  it('arranges the same sessions on a canvas when asked', async () => {
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await startSession(user);
    await waitFor(() => {
      expect(screen.getAllByRole('tab')).toHaveLength(1);
    });

    await arrange(user, 'Canvas');

    // The session is a framed node now, and it is still the same session: the
    // rail still lists it and the details drawer still describes it when
    // opened from the header's Details key (§12: closed by default).
    expect(await screen.findByRole('region', { name: /on codex/i })).toBeInTheDocument();
    expect(screen.getAllByRole('tab')).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: /^details$/i }));
    expect(screen.getByRole('complementary', { name: /session details/i })).toBeInTheDocument();
  });

  it('opens in the arrangement this browser last chose', async () => {
    window.localStorage.setItem('cma:view-mode', 'canvas');
    const user = userEvent.setup();

    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('button', { name: 'View options' }));
    expect(screen.getByRole('menuitemradio', { name: 'Canvas' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('keeps showing a session that ended on its own', async () => {
    // The record and history still exist, so it moves into the closed
    // Terminated group while the open surface keeps showing what it last said.
    FakeEventStream.outcome = 'terminated';
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await startSession(user);

    await waitFor(() => {
      expect(screen.queryAllByRole('tab')).toHaveLength(0);
    });
    expect(screen.getByRole('button', { name: 'Terminated, 1 session' })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: /^Session .* on codex$/ })).toBeInTheDocument();
    expect(screen.queryByText('Attaching…')).toBeNull();
  });

  it('shows the sign-in a session is waiting on, instead of a starting message', async () => {
    /*
     * #355. A kernel spending its owner's own account stops inside the
     * conversation open to ask them to sign in, so the session is still
     * `provisioning` — and that lifecycle drew a spinner reading "this can
     * take a minute", indefinitely, for something that is not taking time and
     * will never finish on its own. The request was arriving on the stream the
     * whole time and being dropped.
     */
    FakeEventStream.signIn = {
      url: 'https://auth.example.test/device',
      code: 'WDJB-MJHT',
      message: 'Sign in to continue',
    };
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await startSession(user);

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Sign in to continue' })).toBeInTheDocument();
    });
    expect(screen.getByText('WDJB-MJHT')).toBeInTheDocument();
    // The starting message is not merely redundant here, it is wrong: it tells
    // somebody to wait for the thing they are being asked to do.
    expect(screen.queryByText(/Starting a sandbox for this session/)).toBeNull();
  });

  it('clears the sign-in once the session moves on', async () => {
    // A completed sign-in leaves a code that has already expired. Showing it
    // afterwards asks somebody to enter something that cannot work.
    FakeEventStream.signIn = { url: 'https://auth.example.test/device', code: 'WDJB-MJHT' };
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await startSession(user);

    await waitFor(() => {
      expect(screen.getByText('WDJB-MJHT')).toBeInTheDocument();
    });

    const stream = streams[streams.length - 1];
    if (stream === undefined) {
      throw new Error('the progress stream was never opened');
    }
    await act(async () => {
      stream.announce('ready');
    });

    await waitFor(() => {
      expect(screen.queryByText('WDJB-MJHT')).toBeNull();
    });
  });

  it('ignores a stored arrangement this build does not have', async () => {
    // Untrusted input: an older build, or another script on the origin, may
    // have written it. Falling through to the list is what keeps the console
    // from rendering a mode that does not exist.
    window.localStorage.setItem('cma:view-mode', 'split-screen');
    const user = userEvent.setup();

    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('button', { name: 'View options' }));
    expect(screen.getByRole('menuitemradio', { name: 'List' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('reconnects one session without redialling the rest of the canvas', async () => {
    seedSessions(3);
    window.localStorage.setItem('cma:view-mode', 'canvas');
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await waitFor(() => {
      expect(sockets).toHaveLength(3);
    });

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^reconnect/i }));

    /*
     * One more socket, not four more. Reconnect is a per-session button in a
     * per-session panel; rebuilding the canvas would clear every scrollback on
     * it and redial every session for one session's hiccup.
     */
    await waitFor(() => {
      expect(sockets).toHaveLength(4);
    });
  });

  it('keeps the terminal attached when the details drawer opens and closes', async () => {
    // §12 puts the drawer beside the surface, so opening it changes the
    // surface's width and nothing else. Rendered around the terminal instead
    // of beside it, a toggle would remount xterm and take the scrollback with
    // it — which is the whole cost of getting this wrong, and is invisible
    // until somebody scrolls up and finds nothing.
    seedSessions(1);
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await waitFor(() => {
      expect(sockets).toHaveLength(1);
    });

    await user.click(screen.getByRole('button', { name: /^details$/i }));
    await user.click(screen.getByRole('button', { name: /close session details/i }));

    // One socket still, not three. A remounted terminal redials.
    expect(sockets).toHaveLength(1);
  });

  it('reports a failed action without detaching any of twelve live terminals', async () => {
    // §12's own worked example, at the size that makes it matter: the canvas
    // attaches its whole budget, one session's stop comes back an error, and
    // nothing else may lose its connection to say so.
    seedSessions(MAX_LIVE_NODES);
    window.localStorage.setItem('cma:view-mode', 'canvas');
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await waitFor(() => {
      expect(sockets).toHaveLength(MAX_LIVE_NODES);
    });

    // Nothing stubs the stop endpoint, so the request comes back 404 — the
    // same shape as a 409, an expired token or a network blip. End lives in
    // the header's overflow menu now (§7).
    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^pause session/i }));

    const alert = await screen.findByRole('alert');
    expect(alert).toBeInTheDocument();
    // A dismissible card at the head of the column, not a sentence in place
    // of the surface (§12). It names what was not changed and the API's code.
    expect(alert).toHaveClass('notice--fail');
    expect(alert.textContent).toContain('Pausing the session failed.');
    expect(alert.textContent).toContain('The session keeps running.');
    expect(alert.textContent).toContain('HTTP 404');
    expect(within(alert).getByRole('button', { name: /dismiss/i })).toBeInTheDocument();

    /*
     * Twelve sessions attached, one request failed. Rendering the notice in
     * place of the surface unmounted the canvas, so eleven sessions that had
     * not failed lost their terminals and their connections to report
     * something that was not about them.
     */
    expect(document.querySelectorAll('[data-node-id]')).toHaveLength(MAX_LIVE_NODES);
    for (let i = 0; i < MAX_LIVE_NODES; i += 1) {
      expect(isAttached(`sesn_${String(i)}`), `sesn_${String(i)}`).toBe(true);
    }
    // And no session was redialled to report the failure.
    expect(sockets).toHaveLength(MAX_LIVE_NODES);
  });

  it('clears the failure as the next action starts', async () => {
    // §12: the band reports the most recent attempt, so a stale failure never
    // sits above a console that has since recovered.
    seedSessions(1);
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^pause session/i }));
    expect(await screen.findByRole('alert')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^reconnect/i }));
    await waitFor(() => {
      expect(screen.queryByRole('alert')).toBeNull();
    });
  });

  it('keeps the node that inherits focus attached when a session is deleted', async () => {
    /*
     * The canvas spends its attachment budget from the focus history, so a
     * session promoted to focused without entering that history is evicted the
     * moment the history is full — the one node §15 says is never detached.
     */
    seedSessions(MAX_LIVE_NODES + 2);
    window.localStorage.setItem('cma:view-mode', 'canvas');
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await waitFor(() => {
      expect(document.querySelectorAll('[data-node-id]')).toHaveLength(MAX_LIVE_NODES + 2);
    });

    // Every session except the first is put ahead of it in the history, so
    // recency alone exhausts the budget once the focused one is gone.
    for (let i = 1; i < MAX_LIVE_NODES + 2; i += 1) {
      fireEvent.click(within(canvasNode(`sesn_${i}`)).getByRole('button'));
    }
    // Delete is in the session's lifecycle menu (§12), gated behind an
    // acknowledgement (§7).
    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^delete/i }));
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: /delete permanently/i }));

    await waitFor(() => {
      expect(document.querySelectorAll('[data-node-id]')).toHaveLength(MAX_LIVE_NODES + 1);
    });
    expect(within(canvasNode('sesn_0')).getByRole('button')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(isAttached('sesn_0')).toBe(true);
  });

  it('does not flash a stream error while a provisioning session is deliberately deleted', async () => {
    FakeEventStream.signIn = {
      url: 'https://auth.example.test/device',
      message: 'Sign in to continue',
    };
    let finishDelete!: (response: Response) => void;
    deleteResponse = new Promise((resolve) => {
      finishDelete = resolve;
    });
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    await startSession(user);
    await waitFor(() => {
      expect(streams).toHaveLength(1);
    });

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^delete/i }));
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: /delete permanently/i }));
    await waitFor(() => {
      expect(deleteRequested).toBe(true);
    });

    act(() => {
      streams[0]?.close(new Error('event stream closed during requested deletion'));
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.queryByRole('alert')).toBeNull();

    finishDelete(new Response(null, { status: 204 }));
    await waitFor(() => {
      expect(screen.queryByRole('tab')).toBeNull();
    });
  });

  it('restores the owner-visible session named by a canonical deep link', async () => {
    seedSessions(2);
    window.history.replaceState(null, '', '/sessions/sesn_1');

    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    const requested = screen.getByRole('tab', { name: /14 Aug 11:01/ });
    expect(requested).toHaveAttribute('aria-selected', 'true');
    expect(window.location.pathname).toBe('/sessions/sesn_1');
    expect(await screen.findByRole('button', { name: 'Manage lifecycle' })).toBeEnabled();
  });

  it('opens GitHub Triggers from a deep link by replacing it with the home frame', async () => {
    seedSessions(1);
    window.history.replaceState(null, '', '/sessions/sesn_0');
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('menuitem', { name: 'GitHub Triggers' }));

    expect(await screen.findByRole('heading', { name: 'GitHub Triggers' })).toBeInTheDocument();
    expect(window.location.pathname).toBe('/');

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('menuitem', { name: 'Back to sessions' }));
    expect(await screen.findByRole('button', { name: 'Manage lifecycle' })).toBeInTheDocument();
  });

  it('keeps an unknown or wrong-owner session route as a typed read-only view', async () => {
    seedSessions(1);
    window.history.replaceState(null, '', '/sessions/sesn_not_owned');

    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'This session does not exist or is not available to this account.',
    );
    expect(screen.getByRole('tab')).toHaveAttribute('aria-selected', 'false');
    expect(sockets).toHaveLength(0);
    expect(window.location.pathname).toBe('/sessions/sesn_not_owned');
  });

  it('writes session selections to history and restores them with browser Back', async () => {
    seedSessions(2);
    window.history.replaceState(null, '', '/sessions/sesn_0');
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('tab', { name: /14 Aug 11:01/ }));
    expect(window.location.pathname).toBe('/sessions/sesn_1');
    expect(screen.getByRole('tab', { name: /14 Aug 11:01/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );

    await act(async () => {
      window.history.back();
    });
    await waitFor(() => {
      expect(window.location.pathname).toBe('/sessions/sesn_0');
      expect(screen.getByRole('tab', { name: /14 Aug 11:00/ })).toHaveAttribute(
        'aria-selected',
        'true',
      );
    });
  });
});
