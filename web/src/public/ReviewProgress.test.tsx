import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, type Launch, type LaunchStreamOptions } from '@cma/client';
import { PUBLIC_PREVIEW, PUBLIC_RECORD, PUBLIC_TRUST, publicClient } from '../test/public';
import { ReviewCard } from './ReviewCard';
import { Progress } from './Progress';
import { readIntent } from './intent';
import { connectionClient } from '../connections/client';

beforeEach(() => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input) => {
      if (input !== '/auth/session') throw new Error('Unexpected external fetch');
      return new Response(
        JSON.stringify({
          authenticated: true,
          subject: 'owner',
          visitor: true,
          capabilities: ['launch_triggers'],
        }),
        { headers: { 'Content-Type': 'application/json' } },
      );
    }),
  );
});

afterEach(() => {
  expect(vi.mocked(fetch).mock.calls.every(([input]) => input === '/auth/session')).toBe(true);
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

describe('minimal review', () => {
  it.each(['local deadline', 'expired', 'cancelled', 'failed'] as const)(
    'keeps the original Draft unstarted after %s until a new user action',
    async (ending) => {
      vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', 'true');
      vi.useFakeTimers();
      vi.setSystemTime(new Date('2026-11-22T00:00:00.000Z'));
      vi.spyOn(window, 'open').mockReturnValue(null);
      const client = publicClient();
      const record: Launch = {
        ...PUBLIC_RECORD,
        state: 'draft',
        review: { can_start_now: false, needs: [{ type: 'connection', service: 'code' }] },
      };
      const changed = vi.fn();
      let connected = false;
      let activeState: 'expired' | 'cancelled' | 'failed' | undefined;
      vi.spyOn(connectionClient, 'check').mockImplementation(async () => ({
        owner: 'owner',
        ready: connected,
        checked_at: new Date().toISOString(),
        active_state: activeState,
        services: [
          {
            service: 'code',
            display_name: 'Code host',
            kind: 'nyxid',
            state: connected ? 'satisfied' : 'missing',
            connection_mode: 'hosted',
          },
        ],
      }));
      vi.spyOn(connectionClient, 'start').mockImplementation(async (_target, active) => ({
        ...active,
        owner: 'owner',
        hosted_url: 'https://identity.example.test/connect',
        expires_at: new Date(
          Date.now() + (ending === 'local deadline' ? 1000 : 600000),
        ).toISOString(),
      }));
      render(
        <ReviewCard
          expectedSubject="owner"
          client={client}
          record={record}
          onChanged={changed}
          onClose={vi.fn()}
        />,
      );
      await act(async () => undefined);
      fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
      await act(async () => undefined);
      expect(connectionClient.start).toHaveBeenCalledOnce();
      if (ending !== 'local deadline') activeState = ending;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      expect(screen.getByRole('alert')).toHaveTextContent('This sign-in did not finish.');
      expect(client.requests.start).not.toHaveBeenCalled();
      connected = true;
      activeState = undefined;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      expect(client.requests.start).not.toHaveBeenCalled();
      expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
      expect(changed).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(4000);
      });
      expect(client.requests.start).toHaveBeenCalledOnce();
      expect(client.requests.start).toHaveBeenCalledWith(
        record.id,
        expect.anything(),
        expect.anything(),
      );
      expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
    },
  );
  it('continues the same Draft through Start once a hosted connection is satisfied', async () => {
    vi.useFakeTimers();
    vi.spyOn(window, 'open').mockReturnValue(null);
    const client = publicClient();
    const record: Launch = {
      ...PUBLIC_RECORD,
      state: 'draft',
      review: { can_start_now: false, needs: [{ type: 'connection', service: 'code' }] },
    };
    vi.mocked(client.requests.requirements).mockResolvedValue({
      services: [
        { service: 'code', state: 'missing', display_name: 'Code host', connection_mode: 'hosted' },
      ],
      checked_at: record.created_at,
    });
    const changed = vi.fn();
    let connected = false;
    vi.mocked(connectionClient.check).mockImplementation(async () => ({
      owner: 'owner',
      ready: connected,
      checked_at: record.created_at,
      services: [
        {
          service: 'code',
          display_name: 'Code host',
          kind: 'nyxid',
          state: connected ? 'satisfied' : 'missing',
          connection_mode: 'hosted',
        },
      ],
    }));
    vi.spyOn(connectionClient, 'start').mockImplementation(async (_target, active) => ({
      ...active,
      owner: 'owner',
      hosted_url: 'https://identity.example.test/connect',
      expires_at: new Date(Date.now() + 600000).toISOString(),
    }));
    render(
      <ReviewCard
        expectedSubject="owner"
        client={client}
        record={record}
        onChanged={changed}
        onClose={vi.fn()}
      />,
    );
    await act(async () => undefined);
    fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
    await act(async () => undefined);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(client.requests.start).not.toHaveBeenCalled();
    connected = true;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(client.requests.start).toHaveBeenCalledOnce();
    expect(client.requests.start).toHaveBeenCalledWith(
      record.id,
      { trust_confirmed_digest: undefined, acknowledgements: [] },
      { idempotencyKey: expect.any(String) },
    );
    expect(changed).toHaveBeenCalledWith(PUBLIC_RECORD);
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  });
  it('shows only server needs, preserves commands and env data, and submits matching trust', async () => {
    const client = publicClient();
    const changed = vi.fn();
    const actor = userEvent.setup();
    render(
      <ReviewCard
        expectedSubject="owner"
        client={client}
        record={PUBLIC_TRUST}
        preview={PUBLIC_PREVIEW}
        onChanged={changed}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
    expect(
      screen.queryByRole('heading', { name: 'Review before continuing' }),
    ).not.toBeInTheDocument();
    expect(screen.getByText('PROJECT_PATH')).toBeInTheDocument();
    expect(screen.getByText('/home/cma/project_data')).toBeInTheDocument();
    expect(document.querySelector('pre code')?.textContent).toBe(
      PUBLIC_TRUST.trust.disclosures[0]?.commands[0],
    );
    await actor.click(screen.getByRole('checkbox'));
    await actor.click(screen.getByRole('button', { name: 'Continue' }));
    expect(client.requests.start).toHaveBeenCalledWith(
      PUBLIC_RECORD.id,
      { trust_confirmed_digest: 'approved-one', acknowledgements: [] },
      { idempotencyKey: expect.any(String) },
    );
    await waitFor(() => expect(changed).toHaveBeenCalledWith(PUBLIC_RECORD));
  });

  it.each(['transport', 'idempotency_in_flight', 'idempotency_recovery_required'])(
    'freezes trust body and key while Start outcome %s is unknown',
    async (code) => {
      const client = publicClient();
      vi.mocked(client.requests.start)
        .mockRejectedValueOnce(
          code === 'transport' ? new Error('offline') : new ApiError(409, 'Unknown', code),
        )
        .mockResolvedValue(PUBLIC_RECORD);
      const actor = userEvent.setup();
      render(
        <ReviewCard
          expectedSubject="owner"
          client={client}
          record={PUBLIC_TRUST}
          onChanged={vi.fn()}
          onClose={vi.fn()}
        />,
      );
      await actor.click(screen.getByRole('checkbox'));
      await actor.click(screen.getByRole('button', { name: 'Continue' }));
      expect(await screen.findByRole('button', { name: 'Try again' })).toBeEnabled();
      expect(screen.getByRole('checkbox')).toBeDisabled();
      await actor.click(screen.getByRole('button', { name: 'Try again' }));
      const calls = vi.mocked(client.requests.start).mock.calls;
      expect(calls).toHaveLength(2);
      expect(calls[1]).toEqual(calls[0]);
    },
  );

  it('keeps ordered needs and refreshes authoritative requirements after a refusal', async () => {
    const client = publicClient();
    const record: Launch = {
      ...PUBLIC_TRUST,
      review: {
        can_start_now: false,
        needs: [
          { type: 'owner_requires_review' },
          { type: 'connection', service: 'code' },
          ...PUBLIC_TRUST.review.needs,
        ],
      },
    };
    vi.mocked(client.requests.requirements).mockResolvedValue({
      services: [
        {
          service: 'code',
          state: 'missing',
          display_name: 'Code host',
          connection_mode: 'manual',
          setup_url: 'https://identity.example.test/services',
        },
      ],
      checked_at: record.created_at,
    });
    vi.mocked(client.requests.start).mockRejectedValue(
      new ApiError(409, 'Missing', 'launch_requirements_not_met'),
    );
    vi.mocked(client.requests.read).mockResolvedValue(record);
    const changed = vi.fn();
    const actor = userEvent.setup();
    render(
      <ReviewCard
        expectedSubject="owner"
        client={client}
        record={record}
        preview={PUBLIC_PREVIEW}
        onChanged={changed}
        onClose={vi.fn()}
      />,
    );
    await screen.findByRole('heading', { name: 'NyxID services' });
    const headings = screen.getAllByRole('heading', { level: 3 }).map((node) => node.textContent);
    expect(headings).toContain('Install software');
    expect(headings).toContain('Model subscription');
    expect(headings).toContain('Review before continuing');
    await actor.click(screen.getByRole('checkbox'));
    await actor.click(screen.getByRole('button', { name: 'Continue' }));
    expect(await screen.findByText('Connect the accounts below to continue.')).toBeInTheDocument();
    await waitFor(() => expect(changed).toHaveBeenCalledWith(record));
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  });
});

describe('durable progress', () => {
  it('keeps missing accounts blocked until a durable completion is observed', async () => {
    const client = publicClient();
    let stream: LaunchStreamOptions | undefined;
    const waiting: Launch = {
      ...PUBLIC_RECORD,
      steps: PUBLIC_RECORD.steps.map((step) =>
        step.key === 'checking_requirements'
          ? {
              key: step.key,
              status: 'blocked',
              safe_detail: 'Connect required accounts to continue',
              started_at: PUBLIC_RECORD.created_at,
            }
          : step,
      ),
    };
    vi.mocked(client.requests.read).mockResolvedValue(waiting);
    vi.mocked(client.requests.stream).mockImplementation(async (_id, options) => {
      stream = options;
      await new Promise(() => undefined);
    });
    render(<Progress client={client} id={waiting.id} onNavigate={vi.fn()} />);
    await screen.findByText('Connect required accounts to continue');
    await waitFor(() => expect(stream).toBeDefined());
    const row = screen.getByRole('region', { name: 'NyxID Service Connection Verification' });
    expect(row).toHaveTextContent('Needs you');
    expect(row).not.toHaveTextContent('Done');
    const checked: Launch = {
      ...waiting,
      revision: 2,
      event_head: 2,
      steps: waiting.steps.map((step) =>
        step.key === 'checking_requirements'
          ? {
              ...step,
              status: 'completed',
              safe_detail: 'Required accounts are connected',
              completed_at: waiting.updated_at,
            }
          : step,
      ),
    };
    vi.mocked(client.requests.read).mockResolvedValue(checked);
    await act(async () => {
      await stream?.onEvent({
        launch_id: waiting.id,
        seq: 2,
        appended_at: waiting.updated_at,
        kind: { type: 'step_changed', step: checked.steps[1]! },
      });
    });
    expect(
      screen.queryByRole('region', { name: 'NyxID Service Connection Verification' }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole('heading', { name: 'Agent key scope configuration ready' }).closest('li'),
    ).toHaveTextContent('Waiting');
    expect(client.requests.start).not.toHaveBeenCalled();
  });

  it('does not acknowledge an event before its refreshed durable record is visible', async () => {
    const client = publicClient();
    let stream: LaunchStreamOptions | undefined;
    vi.mocked(client.requests.stream).mockImplementation(async (_id, options) => {
      stream = options;
      await new Promise(() => undefined);
    });
    render(<Progress client={client} id={PUBLIC_RECORD.id} onNavigate={vi.fn()} />);
    await screen.findByText('Preparing software');
    await waitFor(() => expect(stream).toBeDefined());
    await expect(
      stream?.onEvent({
        launch_id: PUBLIC_RECORD.id,
        seq: 2,
        appended_at: PUBLIC_RECORD.updated_at,
        kind: { type: 'state_changed', from: 'resolving', to: 'ready' },
      }),
    ).rejects.toThrow('has not reached');
    expect(screen.getByText('Preparing software')).toBeInTheDocument();
  });
  it('follows the acknowledged cursor and replaces the route when the server records ready', async () => {
    const client = publicClient();
    let stream: LaunchStreamOptions | undefined;
    vi.mocked(client.requests.stream).mockImplementation(async (_id, options) => {
      stream = options;
      await new Promise(() => undefined);
    });
    const navigate = vi.fn();
    render(<Progress client={client} id={PUBLIC_RECORD.id} onNavigate={navigate} />);
    await screen.findByText('Preparing software');
    await waitFor(() => expect(stream).toBeDefined());
    expect(stream?.after).toBe(PUBLIC_RECORD.event_head);
    const ready: Launch = {
      ...PUBLIC_RECORD,
      state: 'ready',
      event_head: 2,
      resolution: { ...PUBLIC_RECORD.resolution, agent: { state: 'created', id: 'agt_ready' } },
    };
    vi.mocked(client.requests.read).mockResolvedValue(ready);
    await act(async () => {
      await stream?.onEvent({
        launch_id: PUBLIC_RECORD.id,
        seq: 2,
        appended_at: ready.updated_at,
        kind: { type: 'state_changed', from: 'resolving', to: 'ready' },
      });
    });
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith(
        { kind: 'agent', agentId: 'agt_ready' },
        { replace: true },
      ),
    );
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
    expect(client.requests.start).not.toHaveBeenCalled();
  });

  it('retains the last successful view during stream and polling outages', async () => {
    vi.useFakeTimers();
    const client = publicClient();
    vi.mocked(client.requests.stream).mockRejectedValue(new Error('offline'));
    render(<Progress client={client} id={PUBLIC_RECORD.id} onNavigate={vi.fn()} />);
    await act(async () => undefined);
    expect(screen.getByText('Preparing software')).toBeInTheDocument();
    vi.mocked(client.requests.read).mockRejectedValue(new Error('offline'));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(screen.getByText('Preparing software')).toBeInTheDocument();
    expect(screen.getByText(/Updates are unavailable. Last checked/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Check again' })).toBeInTheDocument();
  });

  it('keeps the actual Agent available after first-message delivery fails', async () => {
    const client = publicClient();
    vi.mocked(client.requests.read).mockResolvedValue({
      ...PUBLIC_RECORD,
      state: 'failed_terminal',
      first_message: 'failed',
      resolution: { ...PUBLIC_RECORD.resolution, agent: { state: 'created', id: 'agt_prepared' } },
      steps: [
        ...(['sandbox', 'workspace', 'agent', 'checking_requirements'] as const).map((key) => ({
          key,
          status: 'completed' as const,
          safe_detail: '',
        })),
        {
          key: 'first_message',
          status: 'failed',
          safe_detail: 'The first message could not be delivered',
        },
      ],
      failure: {
        code: 'first_message_failed',
        safe_detail: 'The first message could not be delivered',
        retryable: false,
        occurred_at: PUBLIC_RECORD.updated_at,
      },
    });
    const navigate = vi.fn();
    render(<Progress client={client} id={PUBLIC_RECORD.id} onNavigate={navigate} />);
    await screen.findByRole('button', { name: 'Continue' });
    const phases = within(
      screen.getByRole('list', { name: 'Agent provisioning progress' }),
    ).getAllByRole('listitem');
    expect(phases).toHaveLength(3);
    phases.forEach((phase) => expect(phase).toHaveTextContent('Done'));
    expect(screen.getByRole('region', { name: 'Sending your first message' })).toHaveTextContent(
      "Didn't work",
    );
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(navigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_prepared' });
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
    expect(client.requests.start).not.toHaveBeenCalled();
  });

  it('shows remediation without unsupported retry mutations and starts over as a new intent', async () => {
    const client = publicClient();
    vi.mocked(client.requests.read).mockResolvedValue({
      ...PUBLIC_RECORD,
      state: 'failed_terminal',
      failure: {
        code: 'runtime_unavailable',
        safe_detail: 'Internal Launch checkpoint',
        retryable: false,
        occurred_at: PUBLIC_RECORD.updated_at,
      },
    });
    const navigate = vi.fn();
    const copy = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: copy },
    });
    render(<Progress client={client} id={PUBLIC_RECORD.id} onNavigate={navigate} />);
    await screen.findByRole('button', { name: 'Start over' });
    expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
    expect(screen.queryByText('Internal Launch checkpoint')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Copy support code' }));
    await waitFor(() => expect(copy).toHaveBeenCalledWith(PUBLIC_RECORD.id));
    fireEvent.click(screen.getByRole('button', { name: 'Start over' }));
    expect(readIntent('/t/public-example')).toMatchObject({
      body: { choice: 'new', start_if_ready: true },
      submitted: false,
    });
    expect(navigate).toHaveBeenCalledWith({
      kind: 'public_trigger',
      publicId: 'public-example',
      placementId: undefined,
    });
  });
});
import '../test/connectedAccounts';
