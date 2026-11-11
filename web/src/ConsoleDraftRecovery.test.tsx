import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@chronoai/cma-sdk';
import { IdentityChangedError } from './agent/transport/identity';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from './test/agent';
import { WIRE_AGENT } from './test/rest';
import './test/connectedAccounts';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function fixture() {
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const id = new URL(String(input)).pathname.match(/\/agents\/([^/]+)$/)?.[1] ?? 'agt_1';
      return Response.json({
        ...WIRE_AGENT,
        id,
        workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
        sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
      });
    },
  });
  const readAccessible = vi.spyOn(sdk.agents, 'readAccessible');
  const streams: AbortSignal[] = [];
  const runAgent = vi.fn();
  const client = agentClient({
    resourceViews: sdk,
    readAgent: async (id) => ({ ...AGENT_DETAIL, id }),
    readAgentTranscript: async (id) => ({ ...AGENT_TRANSCRIPT, agentId: id }),
    readAgentUsage: async (id) => ({ ...AGENT_USAGE, agentId: id }),
    runAgent,
    replayAgent: async (_id, options) =>
      new Promise<void>((resolve) => {
        streams.push(options.signal!);
        options.signal?.addEventListener('abort', () => resolve(), { once: true });
      }),
  });
  const show = (viewerId = 'owner', agentId = 'agt_1', selectedClient = client) => (
    <CmaClientProvider client={selectedClient}>
      <Console
        user={{
          id: viewerId,
          handle: viewerId,
          visitor: false,
          capabilities: ['use_own_sandboxes'],
        }}
        route={{ kind: 'agent', agentId }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  return { client, show, readAccessible, streams, runAgent };
}

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
    test.readAccessible.mockImplementation(() => new Promise(() => {}));
    act(() => window.dispatchEvent(new Event('focus')));
    expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
    expect(test.runAgent).not.toHaveBeenCalled();
    unmount();
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
  'does not transfer a retained draft into another %s scope',
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
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('');
    rerender(test.show());
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(
      scope === 'agent' ? 'Private original draft' : '',
    );
    expect(test.runAgent).not.toHaveBeenCalled();
    unmount();
  },
);
