import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@chronoai/cma-sdk';
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
      return Response.json({ ...WIRE_AGENT, id });
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
  'restores the exact unsent draft after a %s access refresh without submitting it',
  async (failure) => {
    vi.useFakeTimers();
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
    expect(screen.getByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
    expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
    expect(test.streams.every((signal) => signal.aborted)).toBe(true);
    test.readAccessible.mockRestore();
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(screen.getByRole('textbox', { name: 'Message' })).not.toBe(composer);
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(draft);
    expect([...document.querySelectorAll('article.message')].map((el) => el.textContent)).toEqual(
      messages,
    );
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
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('');
    expect(test.runAgent).not.toHaveBeenCalled();
    unmount();
  },
);
