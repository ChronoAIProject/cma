import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError } from '@chronoai/cma-sdk';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { creationClient } from '../test/creation';
import { chooseOption } from '../test/select';
import { BotCreate } from './BotCreate';
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
  await user.selectOptions(screen.getByRole('combobox', { name: 'Reasoning effort' }), 'high');
  await user.selectOptions(
    screen.getByRole('combobox', { name: 'Permission level' }),
    'autonomous',
  );
  await user.type(screen.getByLabelText('System prompt'), 'Respond with concise updates.');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await chooseOption(user, 'Admission policy', 'Private chats only');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
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
