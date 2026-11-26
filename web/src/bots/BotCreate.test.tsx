import { chooseOptionValue } from '../test/select';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError } from '@cma/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { creationClient } from '../test/creation';
import { chooseOption } from '../test/select';
import { BotCreate, type CreationIntent } from './BotCreate';
import { useCreationIntent } from './creationIntent';
import { bot, botClient } from './fixtures.test-support';
import type { BotClient } from './model';

function Form({ client, navigate }: { client: BotClient; navigate: () => void }) {
  const { intent, retain } = useCreationIntent('owner');
  return (
    <BotCreate
      client={client}
      profiles={creationClient()}
      intent={intent}
      retain={retain}
      onNavigate={navigate}
    />
  );
}

beforeEach(() => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ items: [] }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    ),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function fill() {
  const user = userEvent.setup();
  await screen.findByRole('button', { name: 'Event Gateway connection' });
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await user.type(screen.getByLabelText('Consumer name'), 'Research bot');
  await chooseOption(user, 'Event Gateway connection', 'Research channel');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await screen.findByRole('heading', { name: 'Configure the Agent inline' });
  await user.type(
    screen.getByLabelText('GitHub repository workspace (optional)'),
    'https://github.com/example/project',
  );
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await chooseOption(user, 'Admission policy', 'Selected senders in private chats');
  await user.type(screen.getByLabelText('Sender IDs'), '111,111');
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await user.clear(screen.getByLabelText('Sender IDs'));
  await user.type(screen.getByLabelText('Sender IDs'), '111,222');
  await user.clear(screen.getByLabelText('Acknowledgement delay (seconds)'));
  await user.type(screen.getByLabelText('Acknowledgement delay (seconds)'), '1.001');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  return user;
}

it('requires all choices and preserves the exact inline create request after an uncertain response', async () => {
  const create = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('connection lost'))
    .mockResolvedValue({ ...bot, state: 'creating' });
  const navigate = vi.fn();
  const client = botClient({ create });
  const view = render(<Form client={client} navigate={navigate} />);
  const user = await fill();
  await user.click(screen.getByRole('button', { name: 'Create consumer' }));
  await screen.findByRole('button', { name: 'Recover creation' });
  view.unmount();
  render(<Form client={client} navigate={navigate} />);
  expect(create).toHaveBeenCalledTimes(1);
  expect(
    screen.getByText('https://github.com/example/project', { selector: 'dd' }),
  ).toBeInTheDocument();
  await user.click(await screen.findByRole('button', { name: 'Recover creation' }));
  expect(create).toHaveBeenCalledTimes(2);
  expect(create.mock.calls[0]?.[0]).toEqual(create.mock.calls[1]?.[0]);
  expect(create.mock.calls[0]?.[0].connection_ref).toBe('connection-fixture');
  expect(create.mock.calls[0]?.[0]).not.toHaveProperty('profile');
  expect(create.mock.calls[0]?.[0].agent).toEqual({
    model: undefined,
    reasoning_effort: 'medium',
    permission_mode: 'guarded',
    system_prompt: '',
    skills: [],
    permissions: { services: [] },
  });
  expect(create.mock.calls[0]?.[1].idempotencyKey).toBe(create.mock.calls[1]?.[1].idempotencyKey);
  expect(create.mock.calls[0]?.[0].admission).toEqual({
    type: 'scoped',
    senders: { type: 'allowlist', ids: ['111', '222'] },
    chats: { type: 'open' },
    groups: 'deny',
  });
  expect(create.mock.calls[0]?.[0].reply.ack_after_ms).toBe(1001);
  expect(navigate).toHaveBeenCalledWith({ kind: 'bot', botId: bot.bot_id });
  expect(sessionStorage.length).toBe(0);
});

it('shows the public problem detail and permits correction after an explicit refusal', async () => {
  render(
    <Form
      client={botClient({
        create: vi
          .fn()
          .mockRejectedValue(
            new ApiError(409, 'This Event Gateway connection already has a route.'),
          ),
      })}
      navigate={vi.fn()}
    />,
  );
  const user = await fill();
  await user.click(screen.getByRole('button', { name: 'Create consumer' }));
  await screen.findByText('This Event Gateway connection already has a route.');
  expect(screen.getByRole('button', { name: 'Back' })).toBeEnabled();
});

it('creates a default-image consumer without contacting the catalog', async () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  const create = vi.fn().mockResolvedValue(bot);
  render(<Form client={botClient({ create })} navigate={vi.fn()} />);
  const user = await fill();
  expect(screen.queryByRole('group', { name: 'Runtime image' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Create consumer' }));
  expect(create.mock.calls[0]?.[0]).not.toHaveProperty('image');
  expect(fetcher).not.toHaveBeenCalled();
});

it('submits image-free inline Agent setup without any profile authoring choice', async () => {
  const create = vi.fn().mockResolvedValue(bot);
  const client = botClient({ create });
  const user = userEvent.setup();
  render(<Form client={client} navigate={vi.fn()} />);
  await screen.findByRole('button', { name: 'Event Gateway connection' });
  await user.type(screen.getByLabelText('Consumer name'), 'Inline worker');
  await chooseOption(user, 'Event Gateway connection', 'Research channel');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await user.type(screen.getByLabelText('Model'), 'gpt-5');
  await chooseOptionValue(user, screen.getByRole('button', { name: 'Reasoning effort' }), 'high');
  await chooseOptionValue(
    user,
    screen.getByRole('button', { name: 'Permission level' }),
    'autonomous',
  );
  await user.type(screen.getByLabelText('System prompt'), 'Respond with concise updates.');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await chooseOption(user, 'Admission policy', 'Private chats only');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(screen.getByText('Agent setup', { selector: 'dt' })).toBeInTheDocument();
  expect(screen.getByText('Sandbox', { selector: 'dt' })).toBeInTheDocument();
  expect(screen.getByText('Workspace', { selector: 'dt' })).toBeInTheDocument();
  expect(screen.queryByText(/profile/i, { selector: 'dt' })).not.toBeInTheDocument();
  expect(screen.queryByText(/materialize|private runtime revisions/i)).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Create consumer' }));
  expect(create.mock.calls[0]?.[0]).toMatchObject({
    connection_ref: 'connection-fixture',
    agent: {
      model: 'gpt-5',
      reasoning_effort: 'high',
      permission_mode: 'autonomous',
      system_prompt: 'Respond with concise updates.',
      skills: [],
      permissions: { services: [] },
    },
  });
  expect(create.mock.calls[0]?.[0]).not.toHaveProperty('profile');
});

it.each([false, true])(
  'retains accurate historical setup labels during recovery (event worker %s)',
  async (eventWorker) => {
    const intent: CreationIntent = {
      key: 'original-consumer-request',
      profileName: eventWorker ? 'Legacy worker' : 'Legacy Agent',
      botName: 'Original channel',
      sandboxProfileName: 'Legacy Sandbox',
      workspaceProfileName: 'Legacy Workspace',
      body: {
        name: 'Original consumer',
        nyxid_bot_id: 'external-original',
        profile: { id: 'agp_old', revision: 2 },
        sandbox_profile: { id: 'sbp_old', revision: 3 },
        workspace_profile: { id: 'wkp_old', revision: 4 },
        ...(eventWorker ? { event_worker_profile: { id: 'ewp_old', revision: 5 } } : {}),
        admission: { type: 'all' },
        reply: { auto_final: true, ack_after_ms: 2000 },
      },
    };
    const create = vi.fn().mockResolvedValue(bot);
    render(
      <BotCreate
        client={botClient({ create })}
        profiles={creationClient()}
        intent={intent}
        retain={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    expect(
      screen.getByText(
        eventWorker ? 'Historical Event Worker Profile' : 'Historical Agent Profile',
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(eventWorker ? 'Legacy worker · Version 5' : 'Legacy Agent · Version 2'),
    ).toBeInTheDocument();
    expect(screen.getByText('Historical Sandbox Profile')).toBeInTheDocument();
    expect(screen.getByText('Legacy Sandbox · Version 3')).toBeInTheDocument();
    expect(screen.getByText('Historical Workspace Profile')).toBeInTheDocument();
    expect(screen.getByText('Legacy Workspace · Version 4')).toBeInTheDocument();
    expect(screen.queryByText('Created from inline Agent setup')).not.toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole('button', { name: 'Recover creation' }));
    expect(create).toHaveBeenCalledWith(
      intent.body,
      expect.objectContaining({ idempotencyKey: intent.key }),
    );
  },
);
