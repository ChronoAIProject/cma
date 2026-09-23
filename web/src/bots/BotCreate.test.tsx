import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import corpus from '../../../api/crates/layer1-public/tests/fixtures/public-contract/event-worker-profiles.json';
import type { EventWorkerProfileRecord } from '@chronoai/cma-sdk';
import { ApiError } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import {
  creationClient,
  CREATION_AGENT,
  CREATION_SANDBOX,
  CREATION_WORKSPACE,
} from '../test/creation';
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
async function fill(custom = false) {
  const user = userEvent.setup();
  await screen.findByRole('button', { name: 'Telegram bot' });
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await user.type(screen.getByLabelText('Bot name'), 'Research bot');
  await chooseOption(user, 'Telegram bot', 'Telegram research');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await user.click(
    await screen.findByRole('button', { name: `Select ${CREATION_AGENT.name} Version 1` }),
  );
  if (custom) {
    await user.click(screen.getByRole('radio', { name: 'Choose my published profiles' }));
    expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
    await user.click(
      await screen.findByRole('button', { name: `Select ${CREATION_SANDBOX.name} Version 1` }),
    );
    expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
    await user.click(
      await screen.findByRole('button', { name: `Select ${CREATION_WORKSPACE.name} Version 1` }),
    );
    for (const checkbox of screen.getAllByRole('checkbox', { name: 'I trust this profile' })) {
      await user.click(checkbox);
    }
  }
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await chooseOption(user, 'Admission policy', 'Selected senders in private chats');
  await user.type(screen.getByLabelText('Telegram sender IDs'), '111,111');
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await user.clear(screen.getByLabelText('Telegram sender IDs'));
  await user.type(screen.getByLabelText('Telegram sender IDs'), '111,222');
  await user.clear(screen.getByLabelText('Acknowledgement delay (seconds)'));
  await user.type(screen.getByLabelText('Acknowledgement delay (seconds)'), '1.001');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  return user;
}
it('requires all choices and preserves the exact create request after an uncertain response', async () => {
  const create = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('connection lost'))
    .mockResolvedValue({ ...bot, state: 'creating' });
  const navigate = vi.fn();
  const client = botClient({ create });
  const view = render(<Form client={client} navigate={navigate} />);
  const user = await fill(true);
  await user.click(screen.getByRole('button', { name: 'Create Bot' }));
  await screen.findByRole('button', { name: 'Recover creation' });
  view.unmount();
  render(<Form client={client} navigate={navigate} />);
  expect(create).toHaveBeenCalledTimes(1);
  await user.click(await screen.findByRole('button', { name: 'Recover creation' }));
  expect(create).toHaveBeenCalledTimes(2);
  expect(create.mock.calls[0]?.[0]).toEqual(create.mock.calls[1]?.[0]);
  expect(create.mock.calls[0]?.[1].idempotencyKey).toBe(create.mock.calls[1]?.[1].idempotencyKey);
  expect(create.mock.calls[0]?.[0].admission).toEqual({
    type: 'scoped',
    senders: { type: 'allowlist', ids: ['111', '222'] },
    chats: { type: 'open' },
    groups: 'deny',
  });
  expect(create.mock.calls[0]?.[0].reply.ack_after_ms).toBe(1001);
  expect(create.mock.calls[0]?.[0].sandbox_profile).toEqual({
    id: CREATION_SANDBOX.revision.profileId,
    revision: 1,
  });
  expect(create.mock.calls[0]?.[0].workspace_profile).toEqual({
    id: CREATION_WORKSPACE.revision.profileId,
    revision: 1,
  });
  expect(navigate).toHaveBeenCalledWith({ kind: 'bot', botId: bot.bot_id });
  expect(sessionStorage.length).toBe(0);
});
it('shows the public problem detail and permits correction after an explicit refusal', async () => {
  render(
    <Form
      client={botClient({
        create: vi
          .fn()
          .mockRejectedValue(new ApiError(409, 'This Telegram bot already has a route.')),
      })}
      navigate={vi.fn()}
    />,
  );
  const user = await fill();
  await user.click(screen.getByRole('button', { name: 'Create Bot' }));
  await screen.findByText('This Telegram bot already has a route.');
  expect(screen.getByRole('button', { name: 'Back' })).toBeEnabled();
});

it('pins the complete worker revision and all three source references in Bot creation', async () => {
  const worker = corpus.cases[0]!.response.body as unknown as EventWorkerProfileRecord;
  const create = vi.fn().mockResolvedValue(bot);
  const base = botClient({ create });
  const client = {
    ...base,
    eventWorkerProfiles: { list: vi.fn().mockResolvedValue({ items: [worker], next_after: null }) },
  } as unknown as BotClient;
  const user = userEvent.setup();
  render(<Form client={client} navigate={vi.fn()} />);
  await screen.findByRole('button', { name: 'Telegram bot' });
  await user.type(screen.getByLabelText('Bot name'), 'Worker bot');
  await chooseOption(user, 'Telegram bot', 'Telegram research');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await user.click(screen.getByRole('radio', { name: 'Event Worker Profile' }));
  await chooseOption(
    user,
    'Event Worker Profile',
    `${worker.source.name} · Revision ${worker.revision}`,
  );
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await chooseOption(user, 'Admission policy', 'Selected senders in private chats');
  await user.type(screen.getByLabelText('Telegram sender IDs'), '111');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await user.click(screen.getByRole('button', { name: 'Create Bot' }));
  expect(create.mock.calls[0]?.[0]).toMatchObject({
    event_worker_profile: { id: worker.id, revision: worker.revision },
    profile: worker.source.agent_profile,
    workspace_profile: worker.source.workspace_profile,
    sandbox_profile: worker.source.sandbox_profile,
  });
});
