import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentProfileContent, ProfileEditorClient } from '../profiles/client';
import { creationClient } from '../test/creation';
import { AGENT_PROFILE, profileClient } from '../test/profiles';
import { chooseOption } from '../test/select';
import { BotNew } from './BotNew';
import { useCreationIntent } from './creationIntent';
import { bot, botClient } from './fixtures.test-support';
import { readFatherReceipt } from './fatherReceipt';
import type { BotClient } from './model';

const draft = { v: 1, name: 'Paper helper', instructions: 'Explain scientific papers clearly.' };
const link = () => {
  window.history.replaceState(
    null,
    '',
    `/bots/new?setup=${encodeURIComponent(JSON.stringify(draft))}`,
  );
};
afterEach(() => {
  window.history.replaceState(null, '', '/');
  vi.restoreAllMocks();
});

function Entry({
  client,
  authoring,
  viewer = 'owner',
  navigate = vi.fn(),
}: {
  client: BotClient;
  authoring?: ProfileEditorClient<AgentProfileContent>;
  viewer?: string;
  navigate?: (route: unknown) => void;
}) {
  const { intent, retain } = useCreationIntent(viewer);
  return (
    <BotNew
      client={client}
      profiles={creationClient()}
      agentProfiles={authoring}
      viewer={viewer}
      viewerLabel={`${viewer}@example.test`}
      intent={intent}
      retain={retain}
      onNavigate={navigate}
    />
  );
}
const consent = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(screen.getByRole('checkbox', { name: /I reviewed these instructions/ }));
};

it('requires current-account review, creates a private profile, then creates the user-selected Bot', async () => {
  link();
  const createProfile = vi.fn(profileClient().createProfile);
  const publishProfile = vi.fn(profileClient().publishProfile);
  const authoring = profileClient({ createProfile, publishProfile });
  const client = botClient();
  const navigate = vi.fn();
  const user = userEvent.setup();
  render(<Entry client={client} authoring={authoring} navigate={navigate} />);
  expect(screen.getByText('owner@example.test')).toBeInTheDocument();
  expect(screen.getByLabelText('Agent instructions')).toHaveValue(draft.instructions);
  expect(screen.getByRole('button', { name: 'Create my agent profile' })).toBeDisabled();
  expect(createProfile).not.toHaveBeenCalled();
  expect(publishProfile).not.toHaveBeenCalled();
  expect(client.bots.available).not.toHaveBeenCalled();
  await user.clear(screen.getByLabelText('Agent instructions'));
  await user.type(screen.getByLabelText('Agent instructions'), 'My reviewed instructions.');
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Create my agent profile' }));
  await screen.findByRole('heading', { name: 'New Bot' });
  expect(createProfile).toHaveBeenCalledWith(
    {
      name: draft.name,
      description: expect.any(String),
      content: {
        thinkingEffort: 'medium',
        permissionLevel: 'guarded',
        skills: [],
        systemPrompt: 'My reviewed instructions.',
      },
    },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  expect(publishProfile).toHaveBeenCalledWith(
    AGENT_PROFILE.id,
    { draftVersion: 1 },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  expect(client.bots.create).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Bot name')).toHaveValue(draft.name);
  await chooseOption(user, 'Telegram bot', 'Telegram research');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  // The newly published private profile is selected; the normal picker still permits a change.
  expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled();
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await chooseOption(user, 'Admission policy', 'Private chats only');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(screen.getByText(/Paper helper · Version 2/)).toBeInTheDocument();
  expect(client.bots.create).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Create Bot' }));
  expect(client.bots.create).toHaveBeenCalledWith(
    expect.objectContaining({
      name: draft.name,
      nyxid_bot_id: 'external-fixture',
      profile: { id: AGENT_PROFILE.id, revision: 2 },
    }),
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  expect(navigate).toHaveBeenCalledWith({ kind: 'bot', botId: bot.bot_id });
  expect(sessionStorage.length).toBe(0);
  expect(window.location.search).toBe('');
});

it('recovers uncertain create and publish across reload with the same keys and no automatic writes', async () => {
  link();
  const createProfile = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('lost create response'))
    .mockImplementation(profileClient().createProfile);
  const publishProfile = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('lost publish response'))
    .mockImplementation(profileClient().publishProfile);
  const authoring = profileClient({ createProfile, publishProfile });
  const client = botClient();
  const user = userEvent.setup();
  let view = render(<Entry client={client} authoring={authoring} />);
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Create my agent profile' }));
  await screen.findByRole('alert');
  const first = readFatherReceipt('owner');
  expect(first?.document).toBeUndefined();
  view.unmount();
  view = render(<Entry client={client} authoring={authoring} />);
  expect(screen.getByLabelText('Agent instructions')).toBeDisabled();
  expect(createProfile).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: 'Recover profile creation' })).toBeDisabled();
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Recover profile creation' }));
  await screen.findByRole('alert');
  expect(readFatherReceipt('owner')?.document?.id).toBe(AGENT_PROFILE.id);
  expect(createProfile.mock.calls[0]?.[1].idempotencyKey).toBe(first?.createKey);
  expect(createProfile.mock.calls[1]?.[1].idempotencyKey).toBe(first?.createKey);
  expect(createProfile.mock.calls[0]?.[0]).toEqual(createProfile.mock.calls[1]?.[0]);
  view.unmount();
  render(<Entry client={client} authoring={authoring} />);
  expect(publishProfile).toHaveBeenCalledTimes(1);
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Recover profile creation' }));
  await screen.findByRole('heading', { name: 'New Bot' });
  expect(createProfile).toHaveBeenCalledTimes(2);
  expect(publishProfile).toHaveBeenCalledTimes(2);
  expect(publishProfile.mock.calls[0]?.[2].idempotencyKey).toBe(first?.publishKey);
  expect(publishProfile.mock.calls[1]?.[2].idempotencyKey).toBe(first?.publishKey);
  expect(readFatherReceipt('owner')?.published?.revision).toBe(2);
});

it('never borrows another account’s pending profile request', async () => {
  link();
  const createProfile = vi.fn().mockRejectedValue(new TypeError('lost response'));
  const authoring = profileClient({ createProfile });
  const client = botClient();
  const user = userEvent.setup();
  const view = render(<Entry client={client} authoring={authoring} />);
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Create my agent profile' }));
  await screen.findByRole('alert');
  view.unmount();
  render(<Entry client={client} authoring={authoring} viewer="other" />);
  expect(screen.getByText('other@example.test')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Create my agent profile' })).toBeDisabled();
  expect(screen.getByLabelText('Agent instructions')).toBeEnabled();
  expect(readFatherReceipt('other')).toBeUndefined();
  expect(createProfile).toHaveBeenCalledTimes(1);
});

it('refuses to publish a profile returned for another owner', async () => {
  link();
  const publishProfile = vi.fn();
  const user = userEvent.setup();
  render(
    <Entry
      client={botClient()}
      authoring={profileClient({
        createProfile: vi.fn().mockResolvedValue({ ...AGENT_PROFILE, owner: 'someone-else' }),
        publishProfile,
      })}
    />,
  );
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Create my agent profile' }));
  await screen.findByRole('alert');
  expect(publishProfile).not.toHaveBeenCalled();
  expect(readFatherReceipt('owner')?.document).toBeUndefined();
});

it('does not dispatch when durable browser storage is unavailable', async () => {
  link();
  const createProfile = vi.fn();
  const user = userEvent.setup();
  render(<Entry client={botClient()} authoring={profileClient({ createProfile })} />);
  vi.spyOn(Object.getPrototypeOf(sessionStorage), 'setItem').mockImplementation(() => {
    throw new Error('full');
  });
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Create my agent profile' }));
  await screen.findByRole('alert');
  expect(createProfile).not.toHaveBeenCalled();
});

it('does not continue publishing after the authenticated surface is unmounted', async () => {
  link();
  let finish: ((value: typeof AGENT_PROFILE) => void) | undefined;
  const createProfile = vi.fn(
    () =>
      new Promise<typeof AGENT_PROFILE>((resolve) => {
        finish = resolve;
      }),
  );
  const publishProfile = vi.fn();
  const user = userEvent.setup();
  const view = render(
    <Entry client={botClient()} authoring={profileClient({ createProfile, publishProfile })} />,
  );
  await consent(user);
  await user.click(screen.getByRole('button', { name: 'Create my agent profile' }));
  await waitFor(() => expect(createProfile).toHaveBeenCalledTimes(1));
  view.unmount();
  await act(async () => finish?.(AGENT_PROFILE));
  expect(publishProfile).not.toHaveBeenCalled();
  expect(readFatherReceipt('owner')?.document).toBeUndefined();
});

it('requires profile authoring permission and rejects invalid setup before reading protected choices', () => {
  link();
  const client = botClient();
  const view = render(<Entry client={client} />);
  expect(screen.getByText(/needs permission to create its own Agent Profiles/)).toBeInTheDocument();
  expect(client.bots.available).not.toHaveBeenCalled();
  view.unmount();
  window.history.replaceState(null, '', '/bots/new?setup=null');
  render(<Entry client={client} authoring={profileClient()} />);
  expect(screen.getByRole('alert')).toHaveTextContent(/setup could not be read/);
  expect(client.bots.available).not.toHaveBeenCalled();
});
