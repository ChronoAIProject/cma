import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { bot, botClient } from './bots/fixtures.test-support';
import { CmaClientProvider } from './api/context';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import type { AuthProvider, AuthState } from './auth/types';
import { agentClient } from './test/agent';
import './test/connectedAccounts';
import { currentService } from './test/services';

const session = {
  authenticated: true,
  subject: 'mock-subject',
  permissions: ['cma-eg:admin'],
  expires_at: Math.floor(Date.now() / 1000) + 3600,
};
function show() {
  return render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider client={agentClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
}
function portalOption(name: 'Agent Manager' | 'Event Gateway') {
  const navigation = within(screen.getByRole('complementary', { name: 'Application navigation' }));
  fireEvent.click(navigation.getByRole('button', { name: /^Service: / }));
  const menu = within(screen.getByRole('menu', { name: 'CMA services' }));
  return menu.getByRole('menuitemradio', { name });
}
afterEach(() => {
  window.history.replaceState({}, '', '/');
  vi.unstubAllGlobals();
});
it('retains the exact Agent draft, Gateway editor and uncertain request across portal switches', async () => {
  window.history.replaceState({}, '', '/agents/agt_1');
  const writes: RequestInit[] = [];
  let rejectWrite!: (error: Error) => void;
  const pending = new Promise<Response>((_, reject) => {
    rejectWrite = reject;
  });
  const fetcher = vi.fn(async (url: string, init: RequestInit = {}) => {
    if (url.endsWith('/auth/session')) return Response.json(session);
    if (url.endsWith('/v1/channels') && init.method === 'POST') {
      writes.push(init);
      return writes.length === 1 ? pending : Response.json({});
    }
    if (url.endsWith('/v1/channels')) return Response.json({ items: [] });
    throw new Error('Unexpected portal request: ' + url);
  });
  vi.stubGlobal('fetch', fetcher);
  // Settle the fixture reads before querying the shell. Polling the retained
  // portals during mocked responses repeats accessibility work on busy workers.
  await act(async () => {
    show();
  });
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Retain this Agent draft' } });
  const gateway = portalOption('Event Gateway');
  await act(async () => {
    fireEvent.click(gateway);
  });
  const workspace = within(screen.getByRole('region', { name: 'Channel workspace' }));
  fireEvent.click(workspace.getByRole('button', { name: 'New channel' }));
  const editor = screen.getByLabelText('Channel name');
  const form = within(editor.closest('form')!);
  fireEvent.change(editor, { target: { value: 'Retained channel' } });
  fireEvent.change(form.getByLabelText('Provider agent key'), {
    target: { value: 'test-only-credential' },
  });
  fireEvent.change(form.getByLabelText('NyxID issuer'), {
    target: { value: 'https://nyxid.example.test' },
  });
  fireEvent.change(form.getByLabelText('Signing key ID'), { target: { value: 'fixture-key' } });
  fireEvent.change(form.getByLabelText('Route IDs 1'), { target: { value: 'fixture-route' } });
  const submit = form.getByRole('button', { name: 'Create channel' });
  expect(submit).toBeEnabled();
  fireEvent.click(submit);
  expect(writes).toHaveLength(1);
  fireEvent.click(portalOption('Agent Manager'));
  expect(window.location.pathname).toBe('/agents/agt_1');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Retain this Agent draft');
  await act(async () => rejectWrite(new TypeError('Gateway response interrupted')));
  fireEvent.click(portalOption('Event Gateway'));
  expect(screen.getByRole('textbox', { name: 'Channel name' })).toBe(editor);
  expect(editor).toHaveValue('Retained channel');
  const retry = screen.getByRole('button', { name: 'Retry same request' });
  expect(retry).toBeEnabled();
  await act(async () => {
    fireEvent.click(retry);
  });
  expect(writes).toHaveLength(2);
  expect(writes[1]?.body).toBe(writes[0]?.body);
  expect(writes[1]?.headers).toEqual(writes[0]?.headers);
  expect(editor).not.toBeInTheDocument();
  expect(fetcher.mock.calls.some(([url]) => url.includes('/image-manager'))).toBe(false);
}, 15_000);

it('discards retained private service drafts when the global account signs out', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      url.endsWith('/auth/session') ? Response.json(session) : Response.json({ items: [] }),
    ),
  );
  show();
  fireEvent.click(
    within(await screen.findByRole('region', { name: 'Channel workspace' })).getByRole('button', {
      name: 'New channel',
    }),
  );
  const editor = screen.getByRole('textbox', { name: 'Channel name' });
  fireEvent.change(editor, { target: { value: 'private draft' } });
  fireEvent.click(screen.getByRole('button', { name: /account menu/i }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Sign out' }));
  await waitFor(() => expect(editor).not.toBeInTheDocument());
});

/** An identity provider whose every answer the test chooses, in order. */
function scriptedIdentity() {
  const owner = (id: string): AuthState => ({
    status: 'authenticated',
    user: { id, handle: id, capabilities: ['use_own_sandboxes'], visitor: false },
  });
  const answers: ((state: AuthState) => void)[] = [];
  const provider: AuthProvider = {
    current: () => new Promise<AuthState>((resolve) => answers.push(resolve)),
    signIn: async () => {},
    signOut: async () => {},
  };
  const answer = async (state: AuthState) => {
    await waitFor(() => expect(answers.length).toBeGreaterThan(0));
    await act(async () => answers.shift()?.(state));
  };
  const recheck = () =>
    act(() => {
      window.dispatchEvent(new Event('focus'));
    });
  return { provider, answer, owner, recheck };
}

async function openChannelDraft(value: string) {
  fireEvent.click(
    within(await screen.findByRole('region', { name: 'Channel workspace' })).getByRole('button', {
      name: 'New channel',
    }),
  );
  const editor = screen.getByRole('textbox', { name: 'Channel name' });
  fireEvent.change(editor, { target: { value } });
  return editor;
}

it('mounts the Gateway once its owner is known, so resolving sign-in keeps an opened editor', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  const fetcher = vi.fn(async (url: string) =>
    url.endsWith('/auth/session') ? Response.json(session) : Response.json({ items: [] }),
  );
  vi.stubGlobal('fetch', fetcher);
  const identity = scriptedIdentity();
  render(
    <AuthContextProvider provider={identity.provider}>
      <CmaClientProvider client={agentClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  expect(await screen.findByRole('status', { name: 'Signing you in' })).toBeVisible();
  expect(screen.queryByRole('region', { name: 'Channel workspace' })).toBeNull();
  expect(fetcher.mock.calls.some(([url]) => url.includes('/event-gateway/'))).toBe(false);

  await identity.answer(identity.owner('mock-subject'));
  const editor = await openChannelDraft('kept draft');
  // A background check that confirms the same owner retains the editor.
  identity.recheck();
  await identity.answer(identity.owner('mock-subject'));
  expect(screen.getByRole('textbox', { name: 'Channel name' })).toBe(editor);
  expect(editor).toHaveValue('kept draft');
});

/** The Gateway's own session, answered for whoever the browser session names. */
function gatewaySessions(subject: string) {
  const gateway: {
    subject: string;
    checks: number;
    failure?: 'unavailable' | 'network' | 'signed out';
  } = { subject, checks: 0 };
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      if (!url.endsWith('/auth/session')) return Response.json({ items: [] });
      gateway.checks += 1;
      if (gateway.failure === 'network') throw new TypeError('Failed to fetch');
      if (gateway.failure === 'unavailable')
        return Response.json({ detail: 'Gateway unavailable' }, { status: 503 });
      if (gateway.failure === 'signed out')
        return Response.json({ detail: 'Signed out' }, { status: 401 });
      return Response.json({ ...session, subject: gateway.subject });
    }),
  );
  /** Waits until the Gateway has checked its session again and the answer has landed. */
  const rechecked = async (after: number) => {
    await waitFor(() => expect(gateway.checks).toBeGreaterThan(after));
    await act(async () => {});
  };
  return { gateway, rechecked };
}

it('keeps a Gateway editor through an identity outage and discards it for another owner', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  const { gateway, rechecked } = gatewaySessions('mock-subject');
  const identity = scriptedIdentity();
  render(
    <AuthContextProvider provider={identity.provider}>
      <CmaClientProvider client={agentClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await identity.answer({ status: 'error', message: 'identity response was not readable' });
  const editor = await openChannelDraft('outage draft');

  // The first known owner adopts what was opened while the identity was unknown,
  // once the Gateway confirms its own session belongs to the same account.
  const before = gateway.checks;
  identity.recheck();
  await identity.answer(identity.owner('mock-subject'));
  await rechecked(before);
  expect(screen.getByRole('textbox', { name: 'Channel name' })).toBe(editor);
  expect(editor).toBeVisible();
  expect(editor).toHaveValue('outage draft');

  // A different known owner does not inherit it.
  identity.recheck();
  await identity.answer(identity.owner('another-subject'));
  await waitFor(() => expect(editor).not.toBeInTheDocument());
});

it('does not show a draft from an identity outage to another account signed in elsewhere', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  const { gateway } = gatewaySessions('first-subject');
  const identity = scriptedIdentity();
  render(
    <AuthContextProvider provider={identity.provider}>
      <CmaClientProvider client={agentClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await identity.answer({ status: 'error', message: 'identity response was not readable' });
  const editor = await openChannelDraft('first account draft');

  // Another tab signs in as someone else, replacing the shared browser session.
  // This tab learns of it from a reconnect, which re-checks only the CMA identity.
  gateway.subject = 'second-subject';
  act(() => {
    window.dispatchEvent(new Event('online'));
  });
  await identity.answer(identity.owner('second-subject'));

  await waitFor(() => expect(editor).not.toBeInTheDocument());
});

for (const failure of ['unavailable', 'network'] as const) {
  it(`withholds one account's Gateway work from another while the Gateway is ${failure}`, async () => {
    window.history.replaceState({}, '', '/event-gateway');
    const { gateway, rechecked } = gatewaySessions('first-subject');
    const identity = scriptedIdentity();
    render(
      <AuthContextProvider provider={identity.provider}>
        <CmaClientProvider client={agentClient()}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>,
    );
    await identity.answer({ status: 'error', message: 'identity response was not readable' });
    const editor = await openChannelDraft('first account draft');
    const channels = screen.getByRole('region', { name: 'Channels' });

    // Another tab signs in as someone else, and the Gateway cannot confirm its
    // session when this tab learns of it.
    gateway.subject = 'second-subject';
    gateway.failure = failure;
    const before = gateway.checks;
    act(() => {
      window.dispatchEvent(new Event('online'));
    });
    await identity.answer(identity.owner('second-subject'));
    const hiddenFromSecond = () => {
      expect(editor).not.toBeVisible();
      expect(screen.queryByRole('textbox', { name: 'Channel name' })).toBeNull();
      expect(screen.queryByRole('region', { name: 'Channels' })).toBeNull();
      expect(channels.closest('[inert]')).not.toBeNull();
    };
    hiddenFromSecond();
    await rechecked(before);
    hiddenFromSecond();
    const retry = await screen.findByRole('button', { name: 'Try again' });
    expect(retry).toBeVisible();

    // Once the Gateway answers, the second account gets its own page.
    gateway.failure = undefined;
    fireEvent.click(retry);
    expect(await screen.findByRole('region', { name: 'Channels' })).not.toBe(channels);
    expect(editor).not.toBeInTheDocument();
  });
}

it('checks the Gateway again when the owner resolves after it refused an unknown viewer', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  const { gateway } = gatewaySessions('mock-subject');
  gateway.failure = 'signed out';
  const identity = scriptedIdentity();
  render(
    <AuthContextProvider provider={identity.provider}>
      <CmaClientProvider client={agentClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await identity.answer({ status: 'error', message: 'identity response was not readable' });
  expect(await screen.findByRole('link', { name: 'Sign in with NyxID' })).toBeVisible();

  gateway.failure = undefined;
  act(() => {
    window.dispatchEvent(new Event('online'));
  });
  await identity.answer(identity.owner('mock-subject'));
  expect(await screen.findByRole('region', { name: 'Channel workspace' })).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Sign in with NyxID' })).toBeNull();
});

it('closes the narrow Gateway drawer after choosing New channel while keeping its editor reachable', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: 390 });
  const bounds = vi
    .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
    .mockReturnValue({ width: 390, height: 844 } as DOMRect);
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      url.endsWith('/auth/session') ? Response.json(session) : Response.json({ items: [] }),
    ),
  );
  try {
    show();
    await screen.findByRole('region', { name: 'Channel workspace' });
    fireEvent.click(await screen.findByRole('button', { name: 'Show navigation' }));
    fireEvent.click(
      within(screen.getByRole('complementary', { name: 'Application navigation' })).getByRole(
        'button',
        {
          name: 'New channel',
        },
      ),
    );
    expect(screen.getByRole('textbox', { name: 'Channel name' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Show navigation' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'New channel' })).not.toBeInTheDocument();
  } finally {
    bounds.mockRestore();
    Object.defineProperty(window, 'innerWidth', {
      configurable: true,
      writable: true,
      value: 1440,
    });
  }
});

// The full shell and first Consumer load can exceed the default timeout on busy CI workers.
it('keeps Consumers under Event Gateway and preserves channel drafts and the Agent when switching sections', async () => {
  window.history.replaceState({}, '', '/agents/agt_1');
  const bots = botClient();
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      url.endsWith('/auth/session') ? Response.json(session) : Response.json({ items: [] }),
    ),
  );
  // Flush the mocked reads with each route change before querying the retained DOM.
  await act(async () => {
    render(
      <AuthContextProvider
        provider={createMockAuthProvider({
          initiallyAuthenticated: true,
          capabilities: ['use_own_sandboxes', 'manage_own_bots'],
        })}
      >
        <CmaClientProvider client={{ ...agentClient(), botViews: bots }}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>,
    );
  });
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Keep my Agent draft' } });
  const gateway = portalOption('Event Gateway');
  await act(async () => fireEvent.click(gateway));
  fireEvent.click(
    within(screen.getByRole('region', { name: 'Channel workspace' })).getByRole('button', {
      name: 'New channel',
    }),
  );
  const channelDraft = screen.getByRole('textbox', { name: 'Channel name' });
  fireEvent.change(channelDraft, { target: { value: 'Retained channel configuration' } });
  const gatewayNavigation = () =>
    within(screen.getByRole('navigation', { name: 'Event Gateway navigation' }));
  const openConsumers = gatewayNavigation().getByRole('button', { name: 'Consumers' });
  await act(async () => fireEvent.click(openConsumers));
  expect(screen.getByRole('region', { name: 'Consumer workspace' })).toBeVisible();
  expect(window.location.pathname).toBe('/bots');
  expect(currentService()).toBe('Service: Event Gateway');
  const consumersNavigation = gatewayNavigation();
  const channels = consumersNavigation.getByRole('button', { name: 'Channels' });
  expect(consumersNavigation.getByRole('button', { name: 'Consumers' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(channels).toHaveAttribute('aria-pressed', 'false');
  const consumer = within(consumersNavigation.getByRole('region', { name: 'Consumers' })).getByRole(
    'button',
    { name: new RegExp(bot.name) },
  );
  // The first consumer selection loads BotPage lazily.
  await act(async () => {
    fireEvent.click(consumer);
    await import('./bots/BotPage');
  });
  expect(screen.getByRole('heading', { name: bot.name })).toBeVisible();
  expect(window.location.pathname).toBe('/bots/' + bot.bot_id);
  expect(currentService()).toBe('Service: Event Gateway');
  fireEvent.click(channels);
  expect(screen.getByRole('textbox', { name: 'Channel name' })).toBe(channelDraft);
  expect(channelDraft).toHaveValue('Retained channel configuration');
  fireEvent.click(portalOption('Agent Manager'));
  expect(window.location.pathname).toBe('/agents/agt_1');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep my Agent draft');
}, 15_000);
