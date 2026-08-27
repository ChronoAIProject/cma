import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ConnectionAuthorization, Launch, TriggerConnection } from '@chronoai/cma-sdk';
import { ApiError } from '../api/client';
import { TriggerReview } from './TriggerReview';

const api = vi.hoisted(() => ({
  beginTriggerConnection: vi.fn(),
  createOrResumeLaunch: vi.fn(),
  listTriggerConnections: vi.fn(),
  preflightTriggerConnection: vi.fn(),
  reconnectTriggerConnection: vi.fn(),
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

const HANDOFF: ConnectionAuthorization = {
  service_id: ACTIVE.service_id,
  authorization_url:
    'https://github.com/login/oauth/authorize?client_id=fixture&state=12345678-1234-4234-8234-123456789abc',
  attempt_nonce: '12345678-1234-4234-8234-123456789abc',
};

function launch(connection: TriggerConnection | null): Launch {
  return { ...FIXTURE, connection };
}

beforeEach(() => {
  vi.clearAllMocks();
  api.createOrResumeLaunch.mockResolvedValue(launch(ACTIVE));
  api.listTriggerConnections.mockResolvedValue([ACTIVE]);
  api.preflightTriggerConnection.mockResolvedValue(ACTIVE);
  api.beginTriggerConnection.mockResolvedValue(HANDOFF);
  api.reconnectTriggerConnection.mockResolvedValue(HANDOFF);
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
    render(<TriggerReview publicId="public-trigger" navigate={vi.fn()} />);

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

  it('reconnects only the selected inactive service without losing edits', async () => {
    const inactive = { ...ACTIVE, state: 'inactive' };
    api.createOrResumeLaunch.mockResolvedValue(launch(inactive));
    api.listTriggerConnections.mockResolvedValue([inactive]);
    api.preflightTriggerConnection.mockRejectedValue(
      new ApiError(409, 'inactive', 'trigger_connection_inactive'),
    );
    render(<TriggerReview publicId="public-trigger" navigate={vi.fn()} />);
    const user = userEvent.setup();

    const task = await screen.findByLabelText(/topic/i);
    await user.clear(task);
    await user.type(task, 'my retained task');
    await user.click(screen.getByRole('button', { name: 'Reconnect' }));

    await waitFor(() =>
      expect(api.reconnectTriggerConnection).toHaveBeenCalledWith(
        ACTIVE.service_id,
        expect.objectContaining({ idempotencyKey: expect.stringMatching(/^cma-web-/) }),
      ),
    );
    expect(task).toHaveValue('my retained task');
  });

  it('manually checks only the selected service', async () => {
    render(<TriggerReview publicId="public-trigger" navigate={vi.fn()} />);
    await screen.findByRole('heading', { name: 'Fix one bounded issue' });
    api.preflightTriggerConnection.mockClear();

    await userEvent.click(screen.getByRole('button', { name: 'Check access' }));

    await waitFor(() =>
      expect(api.preflightTriggerConnection).toHaveBeenCalledWith(ACTIVE.service_id),
    );
  });
});
