import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { bot, botClient } from './bots/fixtures.test-support';
import { CmaClientProvider } from './api/context';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { agentClient } from './test/agent';
import './test/connectedAccounts';

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
  show();
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Retain this Agent draft' } });
  fireEvent.click(screen.getByRole('link', { name: 'Event Gateway' }));
  fireEvent.click(await screen.findByRole('button', { name: 'New channel' }));
  const editor = screen.getByRole('textbox', { name: 'Channel configuration JSON' });
  fireEvent.change(editor, { target: { value: '{"name":"Retained channel","sources":[]}' } });
  fireEvent.change(screen.getByLabelText('Agent key'), {
    target: { value: 'test-only-credential' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await waitFor(() => expect(writes).toHaveLength(1));
  fireEvent.click(screen.getByRole('link', { name: 'Agent Manager' }));
  expect(window.location.pathname).toBe('/agents/agt_1');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Retain this Agent draft');
  await act(async () => rejectWrite(new TypeError('Gateway response interrupted')));
  fireEvent.click(screen.getByRole('link', { name: 'Event Gateway' }));
  expect(screen.getByRole('textbox', { name: 'Channel configuration JSON' })).toBe(editor);
  expect(editor).toHaveValue('{"name":"Retained channel","sources":[]}');
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(writes).toHaveLength(2));
  expect(writes[1]?.body).toBe(writes[0]?.body);
  expect(writes[1]?.headers).toEqual(writes[0]?.headers);
  expect(fetcher.mock.calls.some(([url]) => url.includes('/image-manager'))).toBe(false);
});

it('discards retained private service drafts when the global account signs out', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      url.endsWith('/auth/session') ? Response.json(session) : Response.json({ items: [] }),
    ),
  );
  show();
  fireEvent.click(await screen.findByRole('button', { name: 'New channel' }));
  const editor = screen.getByRole('textbox', { name: 'Channel configuration JSON' });
  fireEvent.change(editor, { target: { value: 'private draft' } });
  fireEvent.click(screen.getByRole('button', { name: /account menu/i }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Sign out' }));
  await waitFor(() => expect(editor).not.toBeInTheDocument());
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
    await screen.findByRole('heading', { name: 'Event Gateway' });
    fireEvent.click(await screen.findByRole('button', { name: 'Show navigation' }));
    fireEvent.click(screen.getByRole('button', { name: 'New channel' }));
    expect(screen.getByRole('textbox', { name: 'Channel configuration JSON' })).toBeVisible();
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

it('keeps Consumers under Event Gateway and preserves channel drafts and the Agent when switching sections', async () => {
  window.history.replaceState({}, '', '/agents/agt_1');
  const bots = botClient();
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      url.endsWith('/auth/session') ? Response.json(session) : Response.json({ items: [] }),
    ),
  );
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
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Keep my Agent draft' } });
  fireEvent.click(screen.getByRole('link', { name: 'Event Gateway' }));
  fireEvent.click(await screen.findByRole('button', { name: 'New channel' }));
  const channelDraft = screen.getByRole('textbox', { name: 'Channel configuration JSON' });
  fireEvent.change(channelDraft, { target: { value: 'Retained channel configuration' } });
  fireEvent.click(screen.getByRole('link', { name: 'Consumers' }));
  await screen.findByRole('heading', { name: 'Event Gateway consumers' });
  expect(window.location.pathname).toBe('/bots');
  expect(screen.getByRole('link', { name: 'Event Gateway' })).toHaveAttribute(
    'aria-current',
    'page',
  );
  expect(screen.getByRole('link', { name: 'Consumers' })).toHaveAttribute('aria-current', 'page');
  fireEvent.click(await screen.findByRole('link', { name: bot.name }));
  await screen.findByRole('heading', { name: bot.name });
  expect(window.location.pathname).toBe('/bots/' + bot.bot_id);
  expect(screen.getByRole('link', { name: 'Event Gateway' })).toHaveAttribute(
    'aria-current',
    'page',
  );
  fireEvent.click(screen.getByRole('link', { name: 'Channels' }));
  expect(screen.getByRole('textbox', { name: 'Channel configuration JSON' })).toBe(channelDraft);
  expect(channelDraft).toHaveValue('Retained channel configuration');
  fireEvent.click(screen.getByRole('link', { name: 'Agent Manager' }));
  expect(window.location.pathname).toBe('/agents/agt_1');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep my Agent draft');
});
