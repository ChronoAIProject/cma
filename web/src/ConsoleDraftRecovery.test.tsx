import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { IdentityChangedError } from './agent/transport/identity';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { AGENT_DETAIL } from './test/agent';
import { consoleDraftFixture as fixture } from './test/consoleDraft.fixture';
import './test/connectedAccounts';
import { agentDraftKey, readAgentDraft } from './agent/draftStorage';
import { AuthContextProvider, useAuth } from './auth/context';
import type { AuthProvider, AuthState } from './auth/types';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('preserves multiple owned drafts through a valid shared Agent visit, return and recreated Console', async () => {
  vi.useFakeTimers();
  const test = fixture();
  await test.share('agt_shared');
  const drafts = new Map([
    ['agt_1', '  First unsent 草稿\n  '],
    ['agt_2', '  Second unsent draft 🧪\n  '],
  ]);
  const view = render(test.show());
  for (const [id, text] of drafts) {
    view.rerender(test.show('owner', id));
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: text },
    });
  }
  view.rerender(test.show('owner', 'agt_shared'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByRole('heading', { name: 'Legitimate shared Agent' })).toBeInTheDocument();
  expect(screen.queryByRole('heading', { name: 'Agent unavailable' })).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  for (const [id, text] of drafts) {
    expect.soft(readAgentDraft('owner', id).content).toBe(text);
    view.rerender(test.show('owner', id));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect.soft(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(text);
  }
  view.unmount();
  const reloaded = render(test.show('owner', 'agt_shared', { ...test.client }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByRole('heading', { name: 'Legitimate shared Agent' })).toBeInTheDocument();
  for (const [id, text] of drafts) {
    reloaded.rerender(test.show('owner', id, { ...test.client }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect.soft(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(text);
  }
  expect(test.runAgent).not.toHaveBeenCalled();
  expect(test.uploadAttachment).not.toHaveBeenCalled();
  reloaded.unmount();
});

it.each([
  ['refresh', true],
  ['reopen', true],
  ['reload', true],
  ['refresh', false],
  ['reopen', false],
] as const)(
  'clears the owner namespace when the same personal Agent becomes shared on %s (saved draft=%s)',
  async (transition, savedDraft) => {
    vi.useFakeTimers();
    const test = fixture();
    let view = render(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    if (savedDraft)
      fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
        target: { value: 'Former personal draft' },
      });
    view.rerender(test.show('owner', 'agt_2'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Another retained owner draft' },
    });
    if (transition === 'refresh') {
      view.rerender(test.show());
      await act(() => vi.advanceTimersByTimeAsync(0));
    }
    await test.share('agt_1');
    if (transition === 'reload') {
      view.unmount();
      view = render(test.show('owner', 'agt_1', { ...test.client }));
    } else if (transition === 'reopen') view.rerender(test.show());
    else act(() => window.dispatchEvent(new Event('focus')));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('heading', { name: 'Legitimate shared Agent' })).toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
    for (const id of ['agt_1', 'agt_2'])
      expect(sessionStorage.getItem(agentDraftKey('owner', id))).toBeNull();
    test.readAccessible.mockRestore();
    view.unmount();
    const restored = render(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('');
    expect(test.runAgent).not.toHaveBeenCalled();
    expect(test.uploadAttachment).not.toHaveBeenCalled();
    restored.unmount();
  },
);

it.each([403, 404])(
  'preserves owned drafts when a different Agent refuses access with %s',
  async (status) => {
    vi.useFakeTimers();
    const test = fixture();
    const view = render(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Unrelated private draft' },
    });
    test.readAccessible.mockRejectedValue(new ApiError(status, 'Agent unavailable'));
    view.rerender(test.show('owner', 'agt_unavailable'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
    expect.soft(readAgentDraft('owner', 'agt_1').content).toBe('Unrelated private draft');
    test.readAccessible.mockRestore();
    view.rerender(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect
      .soft(screen.getByRole('textbox', { name: 'Message' }))
      .toHaveValue('Unrelated private draft');
    expect(test.runAgent).not.toHaveBeenCalled();
    expect(test.uploadAttachment).not.toHaveBeenCalled();
    view.unmount();
  },
);

it('clears a formerly personal draft when deletion is discovered after reload', async () => {
  vi.useFakeTimers();
  const test = fixture();
  const view = render(test.show());
  await act(() => vi.advanceTimersByTimeAsync(0));
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Deleted Agent draft' },
  });
  view.unmount();
  const read = vi.spyOn(test.client, 'readAgent').mockResolvedValue({
    ...AGENT_DETAIL,
    status: 'deleted',
  });
  const deleted = render(test.show('owner', 'agt_1', { ...test.client }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(sessionStorage.getItem(agentDraftKey('owner', 'agt_1'))).toBeNull();
  expect(screen.queryByDisplayValue('Deleted Agent draft')).not.toBeInTheDocument();
  expect(test.runAgent).not.toHaveBeenCalled();
  expect(test.uploadAttachment).not.toHaveBeenCalled();
  deleted.unmount();
  read.mockRestore();
});

it.each(['unavailable', 'network', 'timeout'])(
  'keeps rendered history and the exact draft read-only through a %s access refresh',
  async (failure) => {
    vi.useFakeTimers();
    // Keep relative message ages stable while access-refresh timers advance.
    vi.setSystemTime(new Date('2026-11-10T12:00:00Z'));
    const test = fixture();
    const { unmount } = render(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    const composer = screen.getByRole('textbox', { name: 'Message' });
    const draft = '  Preserve this unsent draft\nwith spacing.  ';
    fireEvent.change(composer, { target: { value: draft } });
    const messages = [...document.querySelectorAll('article.message')].map((el) => el.textContent);
    expect(messages).toHaveLength(2);
    expect(test.streams.length).toBeGreaterThan(0);
    if (failure === 'timeout') test.readAccessible.mockImplementation(() => new Promise(() => {}));
    else
      test.readAccessible.mockRejectedValue(
        failure === 'network'
          ? new TypeError('Failed to fetch')
          : new ApiError(503, 'Temporarily unavailable', 'unavailable'),
      );
    await act(() => vi.advanceTimersByTimeAsync(failure === 'timeout' ? 45_000 : 30_000));
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
    expect(composer).toHaveValue(draft);
    expect(readAgentDraft('owner', 'agt_1').content).toBe(draft);
    expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
    expect(screen.getByRole('button', { name: /^Show Agent details/ })).toBeDisabled();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.getByText(/Checking Agent access/)).toBeInTheDocument();
    fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
    expect(test.runAgent).not.toHaveBeenCalled();
    expect(test.streams.every((signal) => signal.aborted)).toBe(true);
    test.readAccessible.mockRestore();
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(draft);
    expect([...document.querySelectorAll('article.message')].map((el) => el.textContent)).toEqual(
      messages,
    );
    expect(test.runAgent).not.toHaveBeenCalled();
    unmount();
  },
);

it('keeps New agent available while returning to an Agent whose access is being rechecked', async () => {
  vi.useFakeTimers();
  const test = fixture();
  const view = (agentId: string) => (
    <CmaClientProvider client={test.client}>
      <Console
        user={{ id: 'owner', handle: 'owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
        onCreate={vi.fn()}
      />
    </CmaClientProvider>
  );
  const { rerender, unmount } = render(view('agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  rerender(view('agt_2'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  // Returning to a known Agent rechecks its access; that check never settles here.
  test.readAccessible.mockImplementation(() => new Promise(() => {}));
  rerender(view('agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByText(/Checking Agent access/)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'New agent' })).toBeInTheDocument();
  unmount();
});

it.each([401, 403, 404, 'identity'] as const)(
  'removes retained history on explicit %s refusal and does not resurrect it during a retry',
  async (refusal) => {
    vi.useFakeTimers();
    const test = fixture();
    const { unmount } = render(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Private draft' },
    });
    test.readAccessible.mockRejectedValue(new ApiError(503, 'Temporary outage'));
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
    test.readAccessible.mockRejectedValue(
      refusal === 'identity' ? new IdentityChangedError() : new ApiError(refusal, 'Access removed'),
    );
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(screen.getByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
    expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
    expect(sessionStorage.getItem(agentDraftKey('owner', 'agt_1'))).toBeNull();
    test.readAccessible.mockImplementation(() => new Promise(() => {}));
    act(() => window.dispatchEvent(new Event('focus')));
    expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
    expect(test.runAgent).not.toHaveBeenCalled();
    unmount();
    test.readAccessible.mockRestore();
    const restored = render(test.show('owner', 'agt_1', { ...test.client }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('');
    expect(test.runAgent).not.toHaveBeenCalled();
    expect(test.uploadAttachment).not.toHaveBeenCalled();
    restored.unmount();
  },
);

it.each(['viewer', 'agent', 'client'])(
  'discards the retained conversation immediately when its %s changes during uncertainty',
  async (scope) => {
    vi.useFakeTimers();
    const test = fixture();
    const { rerender, unmount } = render(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Private original draft' },
    });
    test.readAccessible.mockRejectedValue(new ApiError(503, 'Temporary outage'));
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
    test.readAccessible.mockImplementation(() => new Promise(() => {}));
    rerender(
      test.show(
        scope === 'viewer' ? 'other-owner' : 'owner',
        scope === 'agent' ? 'agt_2' : 'agt_1',
        scope === 'client' ? { ...test.client } : test.client,
      ),
    );
    expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue('Private original draft')).not.toBeInTheDocument();
    expect(test.runAgent).not.toHaveBeenCalled();
    unmount();
  },
);

it.each(['viewer', 'agent', 'client'])(
  'isolates owner/Agent drafts while allowing a verified replacement client: %s',
  async (scope) => {
    vi.useFakeTimers();
    const test = fixture();
    const { rerender, unmount } = render(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Private original draft' },
    });
    rerender(
      test.show(
        scope === 'viewer' ? 'other-owner' : 'owner',
        scope === 'agent' ? 'agt_2' : 'agt_1',
        scope === 'client' ? { ...test.client } : test.client,
      ),
    );
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(
      scope === 'client' ? 'Private original draft' : '',
    );
    rerender(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(
      scope !== 'viewer' ? 'Private original draft' : '',
    );
    expect(test.runAgent).not.toHaveBeenCalled();
    unmount();
  },
);

it('restores the exact draft after all Console state and its client are recreated', async () => {
  vi.useFakeTimers();
  const test = fixture();
  const first = render(test.show());
  await act(() => vi.advanceTimersByTimeAsync(0));
  const text = '  Entire Console reload\n草稿 🧪  ';
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: text } });
  first.unmount();
  const second = render(test.show('owner', 'agt_1', { ...test.client }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(text);
  expect(test.runAgent).not.toHaveBeenCalled();
  second.unmount();
});

it('waits for verified identity, retains data through identity 503, and clears it on logout/account change', async () => {
  vi.useFakeTimers();
  const test = fixture();
  const signedIn = (id = 'owner'): AuthState => ({
    status: 'authenticated',
    user: {
      id,
      handle: id,
      visitor: false,
      capabilities: ['use_own_sandboxes'],
    },
  });
  const current = vi.fn<AuthProvider['current']>().mockResolvedValue(signedIn());
  const provider: AuthProvider = { current, signIn: async () => {}, signOut: async () => {} };
  function App() {
    const { state, signOut } = useAuth();
    return state.status === 'authenticated' ? (
      <>
        {test.show(state.user.id)}
        <button onClick={() => void signOut()}>Fixture sign out</button>
      </>
    ) : (
      <p>Identity pending</p>
    );
  }
  const show = () => (
    <AuthContextProvider provider={provider}>
      <App />
    </AuthContextProvider>
  );
  const first = render(show());
  await act(() => vi.advanceTimersByTimeAsync(0));
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Private owner draft' },
  });
  first.unmount();
  let verify!: (value: AuthState) => void;
  current.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        verify = resolve;
      }),
  );
  const second = render(show());
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.queryByDisplayValue('Private owner draft')).not.toBeInTheDocument();
  expect(readAgentDraft('owner', 'agt_1').content).toBe('Private owner draft');
  await act(async () => verify(signedIn()));
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Private owner draft');
  current.mockResolvedValue({
    status: 'error',
    reason: 'identity_unavailable',
    message: 'Identity unavailable',
  });
  await act(async () => window.dispatchEvent(new Event('focus')));
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(readAgentDraft('owner', 'agt_1').content).toBe('Private owner draft');
  fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
  expect(test.runAgent).not.toHaveBeenCalled();
  current.mockResolvedValue(signedIn('other-owner'));
  await act(async () => window.dispatchEvent(new Event('focus')));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.queryByDisplayValue('Private owner draft')).not.toBeInTheDocument();
  expect(sessionStorage.getItem(agentDraftKey('owner', 'agt_1'))).toBeNull();
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Other owner draft' },
  });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Fixture sign out' })));
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(sessionStorage.getItem(agentDraftKey('other-owner', 'agt_1'))).toBeNull();
  second.unmount();
});
