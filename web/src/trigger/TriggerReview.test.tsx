import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ConnectionAuthorization, Launch, TriggerConnection } from '@chronoai/cma-sdk';
import { ApiError, TransportError } from '../api/client';
import { TriggerReview } from './TriggerReview';

const api = vi.hoisted(() => ({
  beginTriggerConnection: vi.fn(),
  createOrResumeLaunch: vi.fn(),
  listTriggerConnections: vi.fn(),
  preflightTriggerConnection: vi.fn(),
  readLaunch: vi.fn(),
  reconnectTriggerConnection: vi.fn(),
  revokeTriggerConnection: vi.fn(),
  selectLaunchConnection: vi.fn(),
  startLaunch: vi.fn(),
  updateLaunchOverrides: vi.fn(),
}));

const flow = vi.hoisted(() => ({ authorizeTriggerConnection: vi.fn() }));

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  ...api,
}));

vi.mock('./connectionFlow', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./connectionFlow')>()),
  authorizeTriggerConnection: flow.authorizeTriggerConnection,
}));

const FIXTURE = JSON.parse(
  readFileSync('../sdk/typescript/tests/fixtures/launch.json', 'utf8'),
) as Launch;

const ACTIVE: TriggerConnection = {
  service_id: 'service-exact',
  state_version: 3,
  state: 'active',
  account_id: 42,
  account_node_id: 'U_contributor',
  account_login: 'contributor',
  avatar_url: null,
  granted_scopes: ['public_repo'],
  consent_observed_at: '2026-08-27T00:00:00Z',
};

const INACTIVE: TriggerConnection = {
  ...ACTIVE,
  state_version: ACTIVE.state_version + 1,
  state: 'inactive',
};

const HANDOFF: ConnectionAuthorization = {
  service_id: ACTIVE.service_id,
  authorization_url:
    'https://github.com/login/oauth/authorize?client_id=fixture&state=12345678-1234-4234-8234-123456789abc',
  attempt_nonce: '12345678-1234-4234-8234-123456789abc',
};

function launch(connection: TriggerConnection | null): Launch {
  return { ...FIXTURE, connection };
}

function namedLaunch(name: string, id: string, connection: TriggerConnection | null): Launch {
  const current = launch(connection);
  return {
    ...current,
    id,
    trigger_public_id: `trigger-${id}`,
    revision_content: {
      ...current.revision_content,
      task: { ...current.revision_content.task, public_summary: name },
    },
    overrides: {
      ...current.overrides,
      task_inputs: { ...current.overrides.task_inputs, topic: `${name} topic` },
    },
    effective: { ...current.effective, rendered_prompt: `${name} private prompt` },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function storageText(): string {
  return Array.from({ length: window.localStorage.length }, (_, index) => {
    const key = window.localStorage.key(index);
    return key === null ? '' : (window.localStorage.getItem(key) ?? '');
  }).join('\n');
}

function mutateRetainedReview(change: (retained: Record<string, unknown>) => void): void {
  for (let index = 0; index < window.localStorage.length; index += 1) {
    const key = window.localStorage.key(index);
    if (key === null || !key.startsWith('cma:trigger-command-ledger:')) continue;
    const envelope = JSON.parse(window.localStorage.getItem(key) ?? 'null') as {
      commands?: Array<{ action?: string; retained?: Record<string, unknown> }>;
    };
    const command = envelope.commands?.find((item) => item.action === 'update-review');
    if (command?.retained === undefined) continue;
    change(command.retained);
    window.localStorage.setItem(key, JSON.stringify(envelope));
    return;
  }
  throw new Error('No retained review command was found.');
}

beforeEach(() => {
  vi.clearAllMocks();
  api.createOrResumeLaunch.mockResolvedValue(launch(ACTIVE));
  api.readLaunch.mockResolvedValue(launch(ACTIVE));
  api.listTriggerConnections.mockResolvedValue([ACTIVE]);
  api.preflightTriggerConnection.mockResolvedValue(ACTIVE);
  api.beginTriggerConnection.mockResolvedValue(HANDOFF);
  api.reconnectTriggerConnection.mockResolvedValue(HANDOFF);
  api.revokeTriggerConnection.mockResolvedValue(INACTIVE);
  api.selectLaunchConnection.mockImplementation(async () => launch(ACTIVE));
  api.updateLaunchOverrides.mockImplementation(async () => launch(ACTIVE));
  api.startLaunch.mockImplementation(async () => ({ ...launch(ACTIVE), state: 'preflight' }));
  flow.authorizeTriggerConnection.mockImplementation(async (options) => {
    const handoff = await options.begin();
    options.onService?.(handoff.service_id);
    return ACTIVE;
  });
});

describe('Trigger contributor GitHub authorization', () => {
  it('begins with a bare UUID intent while retaining CMA in the review', async () => {
    api.createOrResumeLaunch.mockResolvedValue(launch(null));
    api.listTriggerConnections.mockResolvedValue([]);
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await userEvent.click(await screen.findByRole('button', { name: 'Connect GitHub' }));

    await waitFor(() => expect(api.beginTriggerConnection).toHaveBeenCalledOnce());
    const [intentId, options] = api.beginTriggerConnection.mock.calls[0] as [
      string,
      { idempotencyKey: string },
    ];
    expect(intentId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i,
    );
    expect(intentId).not.toMatch(/^cma-web-/);
    expect(options.idempotencyKey).toMatch(/^cma-web-/);
    expect(screen.getByRole('heading', { name: 'Fix one bounded issue' })).toBeInTheDocument();
    expect(await screen.findByText('Verified')).toBeInTheDocument();
  });

  it('does not claim CMA can reactivate the selected inactive service', async () => {
    const inactive: TriggerConnection = { ...ACTIVE, state: 'inactive' };
    api.createOrResumeLaunch.mockResolvedValue(launch(inactive));
    api.listTriggerConnections.mockResolvedValue([inactive]);
    api.preflightTriggerConnection.mockRejectedValue(
      new ApiError(409, 'inactive', 'trigger_connection_inactive'),
    );
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    const task = await screen.findByLabelText(/topic/i);
    expect(screen.queryByRole('button', { name: 'Revoke connection' })).toBeNull();
    await user.clear(task);
    await user.type(task, 'my retained task');
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
    expect(
      screen.getByText(/cannot reactivate this inactive UserService.*Enable this exact service/i),
    ).toBeInTheDocument();
    api.preflightTriggerConnection.mockClear();
    await user.click(screen.getByRole('button', { name: 'Check access' }));

    await waitFor(() =>
      expect(api.preflightTriggerConnection).toHaveBeenCalledWith(ACTIVE.service_id),
    );
    expect(api.reconnectTriggerConnection).not.toHaveBeenCalled();
    expect(flow.authorizeTriggerConnection).not.toHaveBeenCalled();
    expect(task).toHaveValue('my retained task');
  });

  it('still renews authorization for the selected disconnected service', async () => {
    const disconnected: TriggerConnection = { ...ACTIVE, state: 'disconnected' };
    api.createOrResumeLaunch.mockResolvedValue(launch(disconnected));
    api.listTriggerConnections.mockResolvedValue([disconnected]);
    api.preflightTriggerConnection.mockRejectedValue(
      new ApiError(409, 'reauthorization required', 'trigger_reauthorization_required'),
    );
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await userEvent.click(await screen.findByRole('button', { name: 'Reconnect' }));

    await waitFor(() =>
      expect(api.reconnectTriggerConnection).toHaveBeenCalledWith(
        ACTIVE.service_id,
        expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
      ),
    );
  });

  it('confirms an exact revocation, preserves edits, and never claims immediate provider denial', async () => {
    api.readLaunch.mockResolvedValue(launch(INACTIVE));
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    const task = await screen.findByLabelText(/topic/i);
    await user.clear(task);
    await user.type(task, 'keep this local review');
    await user.click(screen.getByRole('button', { name: 'Revoke connection' }));

    let dialog = screen.getByRole('dialog', { name: 'Revoke GitHub connection?' });
    expect(dialog).toHaveTextContent('workspace, fork, branch, commits, pull request');
    expect(dialog).toHaveTextContent('bounded credential lifetime expires');
    expect(api.revokeTriggerConnection).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: 'Keep connection' }));
    expect(screen.queryByRole('dialog', { name: 'Revoke GitHub connection?' })).toBeNull();
    expect(api.revokeTriggerConnection).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Revoke connection' }));
    dialog = screen.getByRole('dialog', { name: 'Revoke GitHub connection?' });
    await user.click(within(dialog).getByRole('button', { name: 'Revoke connection' }));

    await waitFor(() =>
      expect(api.revokeTriggerConnection).toHaveBeenCalledWith(
        ACTIVE.service_id,
        expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
      ),
    );
    expect(task).toHaveValue('keep this local review');
    expect(
      await screen.findByText(/cannot reactivate this inactive UserService/i),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
    expect(screen.queryByRole('dialog', { name: 'Revoke GitHub connection?' })).toBeNull();
  });

  it('recovers an unresolved revocation after reload with the same service and command key', async () => {
    api.revokeTriggerConnection
      .mockRejectedValueOnce(new TransportError('response lost'))
      .mockRejectedValueOnce(new TransportError('response lost again'));
    const first = render(
      <TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />,
    );
    const user = userEvent.setup();

    await screen.findByText('Verified');
    await user.click(await screen.findByRole('button', { name: 'Revoke connection' }));
    await user.click(
      within(screen.getByRole('dialog', { name: 'Revoke GitHub connection?' })).getByRole(
        'button',
        { name: 'Revoke connection' },
      ),
    );
    await waitFor(() => expect(api.revokeTriggerConnection).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole('alert')).toHaveTextContent('outcome is unknown');
    expect(api.revokeTriggerConnection.mock.calls[1]).toEqual(
      api.revokeTriggerConnection.mock.calls[0],
    );
    const retainedCall = api.revokeTriggerConnection.mock.calls[1];

    first.unmount();
    api.revokeTriggerConnection.mockResolvedValue(INACTIVE);
    api.readLaunch.mockResolvedValue(launch(INACTIVE));
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    const recovered = await screen.findByRole('dialog', { name: 'Revoke GitHub connection?' });
    await user.click(within(recovered).getByRole('button', { name: 'Reconcile revocation' }));
    await waitFor(() => expect(api.revokeTriggerConnection).toHaveBeenCalledTimes(3));
    expect(api.revokeTriggerConnection.mock.calls[2]).toEqual(retainedCall);
    expect(
      await screen.findByText(/cannot reactivate this inactive UserService/i),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
  });

  it('manually checks only the selected service', async () => {
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    api.preflightTriggerConnection.mockClear();

    await userEvent.click(screen.getByRole('button', { name: 'Check access' }));

    await waitFor(() =>
      expect(api.preflightTriggerConnection).toHaveBeenCalledWith(ACTIVE.service_id),
    );
  });

  it('deep-links to an older started Launch and revalidates it without calling Start again', async () => {
    const navigate = vi.fn();
    const recovery: Launch = {
      ...launch(ACTIVE),
      state: 'auth_required',
      blocked_reason: 'owner_credential_unavailable',
      failure: {
        class: 'owner_credential_unavailable',
        code: 'owner_credential_unavailable',
        detail: 'CMA no longer holds usable sign-in authority for this operation.',
        remediation: 'Sign in again to resume the same Launch.',
        correlation_id: 'owner-credential-expired',
        observed_at: '2026-08-27T00:01:00Z',
      },
      resume_state: 'preflight',
      started_at: '2026-08-27T00:00:00Z',
    };
    const resumed: Launch = {
      ...recovery,
      state: 'preflight',
      blocked_reason: null,
      failure: null,
      resume_state: null,
      revision_number: recovery.revision_number + 1,
    };
    const newerCurrent = { ...launch(ACTIVE), id: 'newer-current-launch' };
    api.createOrResumeLaunch.mockResolvedValue(newerCurrent);
    api.readLaunch.mockResolvedValue(recovery);
    api.selectLaunchConnection.mockResolvedValue(resumed);
    render(
      <TriggerReview ownerId="test-owner" recoveryLaunchId={recovery.id} navigate={navigate} />,
    );

    const resume = await screen.findByRole('button', { name: 'Resume Launch' });
    await waitFor(() => expect(resume).toBeEnabled());
    expect(api.readLaunch).toHaveBeenCalledWith(recovery.id);
    expect(api.createOrResumeLaunch).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Start Launch' })).toBeNull();
    expect(screen.getByLabelText(/topic/i)).toBeDisabled();
    await userEvent.click(resume);

    await waitFor(() => expect(api.selectLaunchConnection).toHaveBeenCalledOnce());
    expect(api.selectLaunchConnection).toHaveBeenCalledWith(
      recovery.id,
      recovery.revision_number,
      ACTIVE.service_id,
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
    expect(api.updateLaunchOverrides).not.toHaveBeenCalled();
    expect(api.startLaunch).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ kind: 'launch', launchId: recovery.id }),
    );
  });

  it('redirects a stale recovery deep link without exposing Start', async () => {
    const navigate = vi.fn();
    const recovered = { ...launch(ACTIVE), id: 'older-recovered-launch', state: 'ready' as const };
    api.readLaunch.mockResolvedValue(recovered);

    render(
      <TriggerReview ownerId="test-owner" recoveryLaunchId={recovered.id} navigate={navigate} />,
    );

    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith(
        { kind: 'launch', launchId: recovered.id },
        { replace: true },
      ),
    );
    expect(api.createOrResumeLaunch).not.toHaveBeenCalled();
    expect(api.startLaunch).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Start Launch' })).toBeNull();
  });

  it('clears a dirty failed review before a recovery route loads', async () => {
    const launchA = namedLaunch('Trigger A', 'launch-a', ACTIVE);
    const connectionB: TriggerConnection = {
      ...ACTIVE,
      service_id: 'service-b',
      account_login: 'contributor-b',
    };
    const launchB: Launch = {
      ...namedLaunch('Trigger B', 'launch-b', connectionB),
      state: 'auth_required',
      blocked_reason: 'owner_credential_unavailable',
      resume_state: 'preflight',
      started_at: '2026-08-27T00:00:00Z',
    };
    const bRead = deferred<Launch>();
    api.createOrResumeLaunch.mockResolvedValue(launchA);
    api.readLaunch.mockReturnValue(bRead.promise);
    api.listTriggerConnections.mockResolvedValueOnce([ACTIVE]).mockResolvedValueOnce([connectionB]);
    api.updateLaunchOverrides.mockRejectedValueOnce(new Error('Trigger A review failed'));
    const view = render(
      <TriggerReview ownerId="test-owner" publicId="trigger-a" navigate={vi.fn()} />,
    );
    const user = userEvent.setup();

    const topic = await screen.findByLabelText(/topic/i);
    await user.clear(topic);
    await user.type(topic, 'dirty Trigger A edit');
    await user.click(screen.getByRole('button', { name: 'Save review' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Trigger A review failed');
    expect(screen.getByRole('button', { name: 'Start Launch' })).toBeDisabled();
    const aPreflightSignal = api.preflightTriggerConnection.mock.calls[0]?.[1] as AbortSignal;

    view.rerender(
      <TriggerReview ownerId="test-owner" recoveryLaunchId={launchB.id} navigate={vi.fn()} />,
    );
    expect(aPreflightSignal.aborted).toBe(true);
    expect(screen.queryByRole('heading', { name: 'Trigger A' })).toBeNull();
    expect(screen.queryByText('dirty Trigger A edit')).toBeNull();
    expect(screen.queryByText('Trigger A review failed')).toBeNull();
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByRole('heading', { name: 'Review Trigger' })).toBeInTheDocument();

    await act(async () => {
      bRead.resolve(launchB);
    });
    expect(await screen.findByRole('heading', { name: 'Trigger B' })).toBeInTheDocument();
    expect(screen.getByLabelText(/topic/i)).toHaveValue('Trigger B topic');
    expect(screen.getByText('contributor-b')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Resume Launch' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Start Launch' })).toBeNull();
    expect(api.startLaunch).not.toHaveBeenCalled();
  });

  it('ignores a late Trigger A review response after Trigger B is current', async () => {
    const pendingA = deferred<Launch>();
    const launchA = namedLaunch('Trigger A', 'launch-a', ACTIVE);
    const launchB = namedLaunch('Trigger B', 'launch-b', ACTIVE);
    api.createOrResumeLaunch.mockImplementation((publicId: string) =>
      publicId === 'trigger-a' ? pendingA.promise : Promise.resolve(launchB),
    );
    const view = render(
      <TriggerReview ownerId="test-owner" publicId="trigger-a" navigate={vi.fn()} />,
    );

    view.rerender(<TriggerReview ownerId="test-owner" publicId="trigger-b" navigate={vi.fn()} />);
    expect(await screen.findByRole('heading', { name: 'Trigger B' })).toBeInTheDocument();
    await act(async () => {
      pendingA.resolve(launchA);
    });

    expect(screen.queryByRole('heading', { name: 'Trigger A' })).toBeNull();
    expect(screen.getByRole('heading', { name: 'Trigger B' })).toBeInTheDocument();
    expect(screen.getByLabelText(/topic/i)).toHaveValue('Trigger B topic');
  });
});

describe('Trigger contributor command reconciliation', () => {
  it('shows the exact server-owned price, runtime, capability, quota, and expiry evidence', async () => {
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    expect(screen.getByText('Zero cost')).toBeInTheDocument();
    expect(screen.getByText('cma-trigger-test-runtime@sha256:admitted')).toBeInTheDocument();
    expect(
      screen.getByText(/conversation, durable conversation, workspace checkpoint/),
    ).toBeInTheDocument();
    expect(screen.getByText('0 of 4 active')).toBeInTheDocument();
    expect(screen.getByText(/4000m CPU, 8192 MiB free at review/)).toBeInTheDocument();
    expect(screen.getByText(/Aug 27, 2026/)).toBeInTheDocument();
  });

  it('names omitted and partially declared quota ceilings without inventing zero capacity', async () => {
    api.createOrResumeLaunch.mockResolvedValue({
      ...launch(ACTIVE),
      review_quota: {
        live_sessions: 2,
        reserved_cpu_millis: 500,
        reserved_memory_mib: 1024,
        free_memory_mib: 4096,
        checked_at: '2026-08-27T00:00:00Z',
      },
    });
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    expect(screen.getByText('2 active · no owner limit declared')).toBeInTheDocument();
    expect(
      screen.getByText('No CPU ceiling declared, 4096 MiB free at review'),
    ).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent('undefined');
  });

  it('labels a wholly undeclared deployment ceiling without manufacturing capacity', async () => {
    api.createOrResumeLaunch.mockResolvedValue({
      ...launch(ACTIVE),
      review_quota: {
        live_sessions: 2,
        reserved_cpu_millis: 500,
        reserved_memory_mib: 1024,
        checked_at: '2026-08-27T00:00:00Z',
      },
    });
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    expect(screen.getByText('No deployment ceiling declared')).toBeInTheDocument();
    expect(screen.queryByText('0m CPU, 0 MiB free at review')).toBeNull();
  });

  it('shows a fixed sign-in return failure without automatically starting the Launch', async () => {
    render(
      <TriggerReview
        ownerId="test-owner"
        publicId="public-trigger"
        authError="expired"
        navigate={vi.fn()}
      />,
    );

    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    expect(screen.getByRole('alert')).toHaveTextContent('Sign-in expired');
    expect(screen.getByRole('alert')).toHaveTextContent('Sign in again to continue.');
    expect(api.startLaunch).not.toHaveBeenCalled();
  });

  it('fails closed when server admission evidence is incomplete', async () => {
    api.createOrResumeLaunch.mockResolvedValue({
      ...launch(ACTIVE),
      runtime_admission: null,
    });
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    const start = await screen.findByRole('button', { name: 'Start Launch' });
    await waitFor(() => expect(start).toBeDisabled());
    expect(screen.getAllByText('Admission unavailable').length).toBeGreaterThan(0);
  });

  it('discloses the locked session kernel before Start', async () => {
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    const kernel = screen.getByText('Kernel').closest('div');
    expect(kernel).not.toBeNull();
    expect(within(kernel as HTMLElement).getByText('Codex')).toBeInTheDocument();
  });

  it('discloses every required GitHub scope class before Start', async () => {
    api.createOrResumeLaunch.mockResolvedValue({
      ...launch(ACTIVE),
      revision_content: {
        ...launch(ACTIVE).revision_content,
        repository_access: {
          ...launch(ACTIVE).revision_content.repository_access,
          required_scope_classes: ['public_repository', 'workflow'],
        },
      },
    });
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    expect(
      screen.getByText('Required GitHub access: Public repositories, Workflow files.'),
    ).toBeInTheDocument();
    const scopeClasses = screen.getByText('Required scope classes').closest('div');
    expect(scopeClasses).not.toBeNull();
    expect(within(scopeClasses as HTMLElement).getByText('public_repository')).toBeInTheDocument();
    expect(within(scopeClasses as HTMLElement).getByText('workflow')).toBeInTheDocument();
  });

  it('requires an explicit same-configuration review refresh after Start admission drifts', async () => {
    api.startLaunch.mockRejectedValueOnce(
      new ApiError(409, 'the reviewed price changed', 'trigger_price_changed'),
    );
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Start Launch' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Start Launch' }),
    );

    expect(await screen.findByRole('alert')).toHaveTextContent(/Admission changed before Start/i);
    expect(screen.getByRole('button', { name: 'Start Launch' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Save review' }));

    await waitFor(() => expect(api.updateLaunchOverrides).toHaveBeenCalledOnce());
    expect(api.updateLaunchOverrides.mock.calls[0]?.[2]).toEqual({
      task_inputs: {},
      optional_checks: [],
    });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start Launch' })).toBeEnabled());
  });

  it('submits only actual overrides while displaying locked effective defaults', async () => {
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    const topic = await screen.findByLabelText(/topic/i);
    expect(topic).toHaveValue('the selected issue');
    expect(screen.getByLabelText('CPU millis')).toHaveValue(500);
    expect(screen.getByLabelText('Timeout seconds')).toHaveValue(3600);
    expect(screen.getByLabelText(/Work label/)).toHaveValue('work');
    await user.clear(topic);
    await user.type(topic, 'fix only this issue');
    await user.click(screen.getByRole('button', { name: 'Save review' }));

    await waitFor(() => expect(api.updateLaunchOverrides).toHaveBeenCalledOnce());
    expect(api.updateLaunchOverrides.mock.calls[0]?.[2]).toEqual({
      task_inputs: { topic: 'fix only this issue' },
      optional_checks: [],
    });
  });

  it('keeps the Git branch Launch-owned when a contributor changes the work label', async () => {
    const base = launch(ACTIVE);
    api.createOrResumeLaunch.mockResolvedValue({
      ...base,
      revision_content: {
        ...base.revision_content,
        override_policy: {
          ...base.revision_content.override_policy,
          editable_fields: [...base.revision_content.override_policy.editable_fields, 'work_label'],
        },
      },
    });
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    const label = await screen.findByLabelText(/Work label/);
    const branch = screen.getByText('Branch').closest('div');
    expect(branch).not.toBeNull();
    expect(branch).toHaveTextContent(`cma/${base.id}`);

    await user.clear(label);
    await user.type(label, 'Documentation fix');
    expect(branch).toHaveTextContent(`cma/${base.id}`);
    expect(branch).not.toHaveTextContent('Documentation fix');
    await user.click(screen.getByRole('button', { name: 'Save review' }));

    await waitFor(() => expect(api.updateLaunchOverrides).toHaveBeenCalledOnce());
    expect(api.updateLaunchOverrides.mock.calls[0]?.[2]).toEqual({
      task_inputs: {},
      work_label: 'Documentation fix',
      optional_checks: [],
    });
  });

  it('requires saving and reviewing recalculated admission before Start', async () => {
    const base = launch(ACTIVE);
    const original: Launch = {
      ...base,
      revision_content: {
        ...base.revision_content,
        override_policy: {
          ...base.revision_content.override_policy,
          editable_fields: [
            ...base.revision_content.override_policy.editable_fields,
            'resource_size',
          ],
        },
      },
    };
    const reviewed: Launch = {
      ...original,
      revision_number: original.revision_number + 1,
      overrides: {
        ...original.overrides,
        resource_size: { cpu_millis: 750, memory_mib: 1024 },
      },
      effective: {
        ...original.effective,
        resource_size: { cpu_millis: 750, memory_mib: 1024 },
        digest: 'f'.repeat(64),
      },
      pricing: {
        ...original.pricing!,
        billing_mode: 'contributor',
        cpu_millis: 750,
        estimated_micros: 9_000_000,
      },
    };
    api.createOrResumeLaunch.mockResolvedValue(original);
    api.updateLaunchOverrides.mockResolvedValue(reviewed);
    api.startLaunch.mockResolvedValue({ ...reviewed, state: 'preflight' });
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    const cpu = await screen.findByLabelText('CPU millis');
    await user.clear(cpu);
    await user.type(cpu, '750');

    expect(screen.getByRole('button', { name: 'Start Launch' })).toBeDisabled();
    expect(screen.getByText(/Save the review to recalculate pricing/i)).toBeInTheDocument();
    expect(api.startLaunch).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Save review' }));
    await waitFor(() => expect(api.updateLaunchOverrides).toHaveBeenCalledOnce());
    await waitFor(() =>
      expect(screen.getByText('Price').parentElement).toHaveTextContent(/9\.00 maximum/),
    );
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start Launch' })).toBeEnabled());
    expect(api.startLaunch).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Start Launch' }));
    await user.click(
      within(screen.getByRole('dialog', { name: 'Start this Launch?' })).getByRole('button', {
        name: 'Start Launch',
      }),
    );

    await waitFor(() => expect(api.startLaunch).toHaveBeenCalledOnce());
    expect(api.startLaunch).toHaveBeenCalledWith(
      reviewed.id,
      reviewed.revision_number,
      reviewed.effective.digest,
      reviewed.workflow_risk_acknowledged,
      expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
    );
  });

  it('hands an expired draft to the retained Launch New Launch recovery flow', async () => {
    const navigate = vi.fn();
    api.startLaunch.mockRejectedValueOnce(
      new ApiError(409, 'the review expired', 'trigger_draft_expired'),
    );
    const current = launch(ACTIVE);
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={navigate} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Start Launch' }));
    await user.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Start Launch' }),
    );

    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ kind: 'launch', launchId: current.id }),
    );
  });

  it.each([
    [404, 'not found', 'trigger_not_found'],
    [422, 'invalid Trigger identifier', 'invalid_trigger_request'],
  ])('does not offer Retry for terminal initial HTTP %i failures', async (status, detail, code) => {
    api.createOrResumeLaunch.mockRejectedValueOnce(new ApiError(status, detail, code));
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    expect(await screen.findByRole('alert')).toHaveTextContent(detail);
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Reconcile Launch' })).toBeNull();
    expect(api.createOrResumeLaunch).toHaveBeenCalledOnce();
  });

  it('offers an explicit Retry for a rate-limited initial read', async () => {
    api.createOrResumeLaunch
      .mockRejectedValueOnce(new ApiError(429, 'Wait before trying again.', 'rate_limited'))
      .mockResolvedValueOnce(launch(ACTIVE));
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    expect(
      await screen.findByRole('heading', { name: 'Trigger review temporarily limited' }),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(
      await screen.findByRole('heading', { name: 'Fix one bounded issue' }),
    ).toBeInTheDocument();
    expect(api.createOrResumeLaunch).toHaveBeenCalledTimes(2);
  });

  it('honors a rate-limit deadline and retains the support reference', async () => {
    api.createOrResumeLaunch.mockRejectedValueOnce(
      new ApiError(
        429,
        'Wait before trying again.',
        'rate_limited',
        'request-rate-limit-668',
        60_000,
      ),
    );
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    expect(await screen.findByText(/request-rate-limit-668/)).toBeInTheDocument();
    expect(screen.getByText(/Retry available after/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
  });

  it('retries a connection-list transport failure instead of labeling it an ambiguous Launch', async () => {
    api.listTriggerConnections
      .mockRejectedValueOnce(new TransportError('connection list unavailable'))
      .mockResolvedValueOnce([ACTIVE]);
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    expect(await screen.findByRole('alert')).toHaveTextContent('connection list unavailable');
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reconcile Launch' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(
      await screen.findByRole('heading', { name: 'Fix one bounded issue' }),
    ).toBeInTheDocument();
  });

  it('reconciles an ambiguous create-or-resume with the original key', async () => {
    api.createOrResumeLaunch.mockRejectedValue(new TransportError('response lost'));
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);

    await waitFor(() => expect(api.createOrResumeLaunch).toHaveBeenCalledTimes(2));
    expect(
      new Set(api.createOrResumeLaunch.mock.calls.map((call) => call[1].idempotencyKey)).size,
    ).toBe(1);

    api.createOrResumeLaunch.mockResolvedValue(launch(ACTIVE));
    await userEvent.click(await screen.findByRole('button', { name: 'Reconcile Launch' }));

    await waitFor(() => expect(api.createOrResumeLaunch).toHaveBeenCalledTimes(3));
    expect(api.createOrResumeLaunch.mock.calls[2]?.[1]).toEqual(
      api.createOrResumeLaunch.mock.calls[0]?.[1],
    );
    expect(
      await screen.findByRole('heading', { name: 'Fix one bounded issue' }),
    ).toBeInTheDocument();
  });

  it('keeps Start confirmation and the exact request after repeated transport loss', async () => {
    const navigate = vi.fn();
    api.startLaunch.mockRejectedValue(new TransportError('response lost'));
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={navigate} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Start Launch' }));
    const dialog = screen.getByRole('dialog', { name: 'Start this Launch?' });
    await user.click(within(dialog).getByRole('button', { name: 'Start Launch' }));

    await waitFor(() => expect(api.startLaunch).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole('alert')).toHaveTextContent(/Start outcome is unknown/i);
    expect(within(dialog).getByRole('button', { name: 'Reconcile Start' })).toBeEnabled();
    expect(screen.getByLabelText(/topic/i)).toBeDisabled();
    expect(api.startLaunch.mock.calls[1]).toEqual(api.startLaunch.mock.calls[0]);

    const started = { ...launch(ACTIVE), state: 'preflight' as const };
    api.startLaunch.mockResolvedValue(started);
    await user.click(within(dialog).getByRole('button', { name: 'Reconcile Start' }));

    await waitFor(() => expect(api.startLaunch).toHaveBeenCalledTimes(3));
    expect(api.startLaunch.mock.calls[2]).toEqual(api.startLaunch.mock.calls[0]);
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ kind: 'launch', launchId: started.id }),
    );
  });

  it('reconstructs a private review after reload only with the original request and key', async () => {
    const privateTopic = 'private reload review text';
    api.updateLaunchOverrides.mockRejectedValue(new TransportError('response lost'));
    const first = render(
      <TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />,
    );
    const user = userEvent.setup();
    const original = await screen.findByLabelText(/topic/i);
    await user.clear(original);
    await user.type(original, privateTopic);
    await user.click(screen.getByRole('button', { name: 'Save review' }));

    await waitFor(() => expect(api.updateLaunchOverrides).toHaveBeenCalledTimes(2));
    const originalKey = api.updateLaunchOverrides.mock.calls[0]?.[3].idempotencyKey as string;
    expect(storageText()).not.toContain(privateTopic);
    first.unmount();

    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const reconstructed = await screen.findByLabelText(/topic/i);
    await screen.findByRole('button', { name: 'Reconcile review' });
    await waitFor(() => expect(reconstructed).toBeEnabled());

    await user.clear(reconstructed);
    await user.type(reconstructed, 'changed private review text');
    await user.click(screen.getByRole('button', { name: 'Reconcile review' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/private text/i);
    expect(api.updateLaunchOverrides).toHaveBeenCalledTimes(2);

    api.updateLaunchOverrides.mockResolvedValue(launch(ACTIVE));
    await user.clear(reconstructed);
    await user.type(reconstructed, privateTopic);
    await user.click(screen.getByRole('button', { name: 'Reconcile review' }));

    await waitFor(() => expect(api.updateLaunchOverrides).toHaveBeenCalledTimes(3));
    expect(api.updateLaunchOverrides.mock.calls[2]?.[3].idempotencyKey).toBe(originalKey);
    expect(api.updateLaunchOverrides.mock.calls[2]?.[2].task_inputs).toEqual({
      topic: privateTopic,
    });
  });

  it('rejects tampered private-review metadata without another API call', async () => {
    const privateTopic = 'private review bound to this launch';
    api.updateLaunchOverrides.mockRejectedValue(new TransportError('response lost'));
    const first = render(
      <TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />,
    );
    const user = userEvent.setup();
    const topic = await screen.findByLabelText(/topic/i);
    await user.clear(topic);
    await user.type(topic, privateTopic);
    await user.click(screen.getByRole('button', { name: 'Save review' }));
    await waitFor(() => expect(api.updateLaunchOverrides).toHaveBeenCalledTimes(2));
    first.unmount();

    mutateRetainedReview((retained) => {
      retained.launchId = 'attacker-selected-launch';
    });
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const reconstructed = await screen.findByLabelText(/topic/i);
    await screen.findByRole('button', { name: 'Reconcile review' });
    await waitFor(() => expect(reconstructed).toBeEnabled());
    await user.clear(reconstructed);
    await user.type(reconstructed, privateTopic);
    await user.click(screen.getByRole('button', { name: 'Reconcile review' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(/current resource/i);
    expect(api.updateLaunchOverrides).toHaveBeenCalledTimes(2);
  });

  it('joins duplicate Start clicks to one in-flight command', async () => {
    let finish!: (value: Launch) => void;
    api.startLaunch.mockImplementation(
      () =>
        new Promise<Launch>((resolve) => {
          finish = resolve;
        }),
    );
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    await user.click(await screen.findByRole('button', { name: 'Start Launch' }));
    const confirm = within(screen.getByRole('dialog')).getByRole('button', {
      name: 'Start Launch',
    });
    fireEvent.click(confirm);
    fireEvent.click(confirm);

    await waitFor(() => expect(api.startLaunch).toHaveBeenCalledOnce());
    finish({ ...launch(ACTIVE), state: 'preflight' });
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('retains review fields when another tab advances the Launch revision', async () => {
    api.updateLaunchOverrides.mockRejectedValue(
      new ApiError(409, 'revision conflict', 'revision_conflict'),
    );
    render(<TriggerReview ownerId="test-owner" publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();
    const topic = await screen.findByLabelText(/topic/i);
    await user.clear(topic);
    await user.type(topic, 'my concurrent edit');
    await user.click(screen.getByRole('button', { name: 'Save review' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(/another tab/i);
    expect(topic).toHaveValue('my concurrent edit');
  });
});
