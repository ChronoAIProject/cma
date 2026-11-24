import { chooseOptionValue, openedOptions } from '../test/select';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent, { type UserEvent } from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { SessionSettings } from './SessionSettings';
import { AgentSettingsError } from '../api/rest/agentSettings';

const defaults = {
  model: 'gpt-6',
  reasoning_effort: 'high',
  permission_mode: 'autonomous',
} as const;
const catalog = async () => [
  { id: 'gpt-6', label: 'GPT-6', available: true },
  { id: 'invalid', label: 'Retired model', available: true },
  { id: 'gpt-6-updated', label: 'Updated model', available: true },
];

async function expandSettings() {
  const summary = await screen.findByRole('button', { name: /^Agent settings:/ });
  if (summary.getAttribute('aria-expanded') === 'false') fireEvent.click(summary);
}

const chip = () => screen.getByRole('button', { name: /^Model and reasoning effort/ });
const card = () => screen.getByRole('dialog', { name: 'Model and reasoning effort' });
const slider = () => within(card()).getByRole('slider', { name: 'Reasoning effort' });
const model = () => within(card()).getByRole('button', { name: 'Model' });

async function openCard(user: UserEvent) {
  if (screen.queryByRole('dialog', { name: 'Model and reasoning effort' }) === null)
    await user.click(chip());
  return card();
}

/** Move the effort slider to a stop with the keyboard, as a person would. */
async function chooseEffort(user: UserEvent, label: string) {
  await openCard(user);
  slider().focus();
  await user.keyboard('{Home}');
  for (let step = 0; step < 5 && slider().getAttribute('aria-valuetext') !== label; step += 1)
    await user.keyboard('{ArrowRight}');
  expect(slider()).toHaveAttribute('aria-valuetext', label);
}

function draw(client: ReturnType<typeof agentClient>, busy = false) {
  return (
    <CmaClientProvider client={client}>
      <SessionSettings agentId="agent-one" active runtimeReady busy={busy} />
    </CmaClientProvider>
  );
}

it('loads actual settings and retains the same update body and key after an uncertain response', async () => {
  const user = userEvent.setup();
  const update = vi
    .fn()
    .mockRejectedValueOnce(new Error('Lost response'))
    .mockResolvedValue({ ...defaults, reasoning_effort: 'medium' });
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: async () => defaults,
        updateAgentSettings: update,
      }),
    ),
  );
  await expandSettings();
  expect(
    await screen.findByRole('button', { name: /^Model and reasoning effort/ }),
  ).toHaveTextContent('GPT-6 High');
  expect(screen.getByRole('button', { name: 'Permission mode' })).toHaveTextContent('Autonomous');
  expect(screen.queryByRole('button', { name: 'Apply settings' })).not.toBeInTheDocument();
  await chooseEffort(user, 'Medium');
  expect(within(card()).getByText('Medium', { selector: 'p' })).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('The update outcome is unconfirmed. Retry the same request to check it.');
  await openCard(user);
  expect(model()).toBeDisabled();
  expect(slider()).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByRole('button', { name: 'Permission mode' })).toBeDisabled();
  await user.click(screen.getByRole('button', { name: 'Retry settings update' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[1]?.[1]).toEqual(update.mock.calls[0]?.[1]);
  expect(update.mock.calls[1]?.[2].idempotencyKey).toBe(update.mock.calls[0]?.[2].idempotencyKey);
  await openCard(user);
  expect(model()).toBeEnabled();
});

it('does not offer an apply operation while a turn is active', async () => {
  const user = userEvent.setup();
  const update = vi.fn();
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: async () => defaults,
        updateAgentSettings: update,
      }),
      true,
    ),
  );
  await expandSettings();
  await chooseEffort(user, 'Low');
  expect(screen.getByRole('button', { name: 'Apply settings' })).toBeDisabled();
  expect(
    screen.getByText('Settings can be changed after the current turn finishes.'),
  ).toBeVisible();
  expect(update).not.toHaveBeenCalled();
});

it('closes the checked model choice without editing the confirmed settings', async () => {
  const user = userEvent.setup();
  const update = vi.fn().mockResolvedValue(defaults);
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: async () => ({ ...defaults, reasoning_effort: 'medium' }),
        updateAgentSettings: update,
      }),
    ),
  );
  await expandSettings();
  await chooseEffort(user, 'High');
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  await openCard(user);
  await chooseOptionValue(user, model(), defaults.model);
  expect(model()).toHaveAttribute('aria-expanded', 'false');
  expect(slider()).toHaveAttribute('aria-valuetext', 'High');
  expect(screen.getByRole('status').textContent).toBe('Settings applied to this Agent.');
  expect(screen.queryByRole('button', { name: 'Apply settings' })).not.toBeInTheDocument();
  expect(update).toHaveBeenCalledTimes(1);
});

it('allows correction after a definitive rejection without replaying its command key', async () => {
  const user = userEvent.setup();
  const update = vi
    .fn()
    .mockRejectedValueOnce(new AgentSettingsError('Invalid model', 422))
    .mockResolvedValue(defaults);
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: async () => defaults,
        updateAgentSettings: update,
      }),
    ),
  );
  await expandSettings();
  await openCard(user);
  await chooseOptionValue(user, model(), 'invalid');
  expect(chip()).toHaveTextContent('Retired model High');
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('The update was rejected. Check your changes, then try again.');
  await openCard(user);
  expect(model()).toBeEnabled();
  await chooseOptionValue(user, model(), 'gpt-6-updated');
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[1]?.[2].idempotencyKey).not.toBe(
    update.mock.calls[0]?.[2].idempotencyKey,
  );
});

it('waits for runtime confirmation when the server reports pending settings', async () => {
  vi.useFakeTimers();
  const read = vi
    .fn()
    .mockResolvedValueOnce({ ...defaults, pending: true })
    .mockResolvedValue({ ...defaults, pending: false });
  render(
    draw(
      agentClient({
        readAgentModels: async () => [{ id: 'gpt-6', label: 'GPT-6', available: true }],
        readAgentSettings: read,
        updateAgentSettings: vi.fn(),
      }),
    ),
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  fireEvent.click(screen.getByRole('button', { name: /^Agent settings:/ }));
  expect(screen.getByText('Waiting for the runtime to confirm these settings.')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Permission mode' })).toBeDisabled();
  fireEvent.click(chip());
  expect(model()).toBeDisabled();
  expect(screen.queryByText('Settings applied to this Agent.')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(
    screen.queryByText('Waiting for the runtime to confirm these settings.'),
  ).not.toBeInTheDocument();
  expect(model()).toBeEnabled();
  vi.useRealTimers();
});

it('loads the scoped model catalog, preserves an unavailable current model, and never invents choices', async () => {
  const user = userEvent.setup();
  const readModels = vi
    .fn()
    .mockRejectedValueOnce(new Error('Catalog unavailable'))
    .mockResolvedValue([{ id: 'available-model', label: 'Available model', available: true }]);
  render(
    draw(
      agentClient({
        readAgentModels: readModels,
        readAgentSettings: async () => defaults,
        updateAgentSettings: vi.fn(),
      }),
    ),
  );
  await expandSettings();
  await openCard(user);
  await within(card()).findByText('Catalog unavailable');
  expect(model()).toBeDisabled();
  await user.click(within(card()).getByRole('button', { name: 'Retry models' }));
  await user.click(model());
  expect(
    await screen.findByRole('menuitemradio', {
      name: /gpt-6 Current model is not in the available catalog/,
    }),
  ).toHaveAttribute('aria-disabled', 'true');
  await user.click(screen.getByRole('menuitemradio', { name: 'Available model' }));
  expect(model()).toHaveTextContent('Available model');
  expect(chip()).toHaveTextContent('Available model High');
});

it('distinguishes look-alike model labels and applies the selected catalog ID', async () => {
  const user = userEvent.setup();
  const saved = { ...defaults, model: 'gpt-6-astra' };
  const selected = { ...defaults, model: 'gpt-5.6-sol-preview' };
  const update = vi.fn().mockResolvedValue(selected);
  render(
    draw(
      agentClient({
        readAgentModels: async () => [
          { id: 'gpt-5.6-sol-preview', label: 'GPT-5.6 (Sol)', available: true },
          { id: 'gpt-5.6-sol', label: 'GPT-5.6 Sol', available: true },
          { id: 'gpt-6-astra', label: 'GPT-6 (Astra)', available: true },
          { id: 'gpt-6-astra-preview', label: 'GPT-6 Astra', available: true },
          { id: 'gpt-6', label: 'GPT-6', available: true },
        ],
        readAgentSettings: async () => saved,
        updateAgentSettings: update,
      }),
    ),
  );
  await expandSettings();
  await openCard(user);
  expect(chip()).toHaveTextContent('GPT-6 (Astra) · gpt-6-astra High');
  await user.click(model());
  expect(
    screen.getByRole('menuitemradio', { name: 'GPT-6 (Astra) · gpt-6-astra' }),
  ).toHaveAttribute('aria-checked', 'true');
  expect(
    screen.getByRole('menuitemradio', { name: 'GPT-6 Astra · gpt-6-astra-preview' }),
  ).toBeVisible();
  expect(screen.getByRole('menuitemradio', { name: 'GPT-5.6 Sol · gpt-5.6-sol' })).toBeVisible();
  expect(screen.getByRole('menuitemradio', { name: 'GPT-6' })).toBeVisible();
  await user.click(
    screen.getByRole('menuitemradio', { name: 'GPT-5.6 (Sol) · gpt-5.6-sol-preview' }),
  );
  expect(chip()).toHaveTextContent('GPT-5.6 (Sol) · gpt-5.6-sol-preview High');
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[0]?.[1]).toEqual(selected);
});

it('keeps retained runtime models disabled while applying reasoning changes to the saved model', async () => {
  const user = userEvent.setup();
  const update = vi.fn().mockResolvedValue({ ...defaults, reasoning_effort: 'medium' });
  render(
    draw(
      agentClient({
        readAgentModels: async () => [
          { id: 'gpt-6', label: 'GPT-6', available: false },
          { id: 'gpt-6-astra', label: 'GPT-6 Astra', available: false },
        ],
        readAgentSettings: async () => defaults,
        updateAgentSettings: update,
      }),
    ),
  );
  await expandSettings();
  await openCard(user);
  await within(card()).findByText(/Model changes are unavailable for this Agent/);
  expect(model()).toHaveTextContent('GPT-6');
  await user.click(model());
  const unsupported = screen.getByRole('menuitemradio', {
    name: /GPT-6 Astra This Agent cannot select/,
  });
  expect(unsupported).toHaveAttribute('aria-disabled', 'true');
  await user.click(unsupported);
  expect(model()).toHaveTextContent('GPT-6');
  await user.keyboard('{Escape}');
  await chooseEffort(user, 'Medium');
  expect(screen.getByRole('button', { name: 'Permission mode' })).toBeEnabled();
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[0]?.[1]).toEqual({ ...defaults, reasoning_effort: 'medium' });
});

it('refreshes capability after a turn and preserves reasoning edits when a model draft becomes unavailable', async () => {
  const user = userEvent.setup();
  const readModels = vi
    .fn()
    .mockResolvedValueOnce([{ id: 'gpt-6-astra', label: 'GPT-6 Astra', available: true }])
    .mockResolvedValue([{ id: 'gpt-6-astra', label: 'GPT-6 Astra', available: false }]);
  const update = vi.fn().mockResolvedValue({ ...defaults, reasoning_effort: 'medium' });
  const client = agentClient({
    readAgentModels: readModels,
    readAgentSettings: async () => defaults,
    updateAgentSettings: update,
  });
  const { rerender } = render(draw(client));
  await expandSettings();
  await openCard(user);
  await chooseOptionValue(user, model(), 'gpt-6-astra');
  await chooseEffort(user, 'Medium');
  rerender(draw(client, true));
  await within(card()).findByText(/Model changes are unavailable for this Agent/);
  expect(model()).toHaveTextContent('gpt-6');
  expect(chip()).toHaveTextContent('gpt-6 Medium');
  rerender(draw(client, false));
  await within(card()).findByText(/Model changes are unavailable for this Agent/);
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[0]?.[1]).toEqual({ ...defaults, reasoning_effort: 'medium' });
  expect(readModels.mock.calls.length).toBeGreaterThanOrEqual(3);
});

it('names the permission modes Guarded and Autonomous while sending the unchanged wire values', async () => {
  const update = vi.fn().mockResolvedValue({ ...defaults, permission_mode: 'guarded' });
  render(
    draw(
      agentClient({
        readAgentModels: async () => [{ id: 'gpt-6', label: 'GPT-6', available: true }],
        readAgentSettings: async () => defaults,
        updateAgentSettings: update,
      }),
    ),
  );
  await expandSettings();
  const actor = userEvent.setup();
  expect(await openedOptions(actor, 'Permission mode')).toEqual(['Guarded', 'Autonomous']);
  await actor.keyboard('{Escape}');
  await chooseOptionValue(
    actor,
    screen.getByRole('button', { name: 'Permission mode' }),
    'guarded',
  );
  expect(screen.getByRole('button', { name: 'Permission mode' })).toHaveTextContent('Guarded');
  await actor.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[0]?.[1]).toEqual({ ...defaults, permission_mode: 'guarded' });
});

it('operates the effort slider from the keyboard and announces each stop by name', async () => {
  const user = userEvent.setup();
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: async () => defaults,
        updateAgentSettings: vi.fn(),
      }),
    ),
  );
  await expandSettings();
  await user.click(chip());
  // Opening the card puts focus on the slider, which starts at the Agent's effort.
  expect(slider()).toHaveFocus();
  expect(slider()).toHaveAttribute('aria-valuetext', 'High');
  expect(slider()).toHaveAttribute('aria-valuenow', '3');
  expect(slider()).toHaveAttribute('aria-valuemax', '4');
  await user.keyboard('{ArrowRight}');
  expect(slider()).toHaveAttribute('aria-valuetext', 'Very high');
  await user.keyboard('{ArrowRight}');
  expect(slider()).toHaveAttribute('aria-valuetext', 'Very high');
  await user.keyboard('{Home}');
  expect(slider()).toHaveAttribute('aria-valuetext', 'Minimal');
  await user.keyboard('{ArrowLeft}');
  expect(slider()).toHaveAttribute('aria-valuetext', 'Minimal');
  await user.keyboard('{PageUp}');
  expect(slider()).toHaveAttribute('aria-valuetext', 'Low');
  await user.keyboard('{End}');
  expect(slider()).toHaveAttribute('aria-valuetext', 'Very high');
  await user.keyboard('{ArrowDown}');
  expect(slider()).toHaveAttribute('aria-valuetext', 'High');
  expect(chip()).toHaveTextContent('GPT-6 High');
  expect(screen.queryByRole('button', { name: 'Apply settings' })).not.toBeInTheDocument();
});

it('snaps a press on the track to the nearest stop', async () => {
  const user = userEvent.setup();
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: async () => defaults,
        updateAgentSettings: vi.fn(),
      }),
    ),
  );
  await expandSettings();
  await user.click(chip());
  vi.spyOn(slider(), 'getBoundingClientRect').mockReturnValue(
    DOMRect.fromRect({ x: 100, y: 0, width: 200, height: 20 }),
  );
  fireEvent.pointerDown(slider(), { button: 0, clientX: 148, pointerId: 1 });
  expect(slider()).toHaveAttribute('aria-valuetext', 'Low');
  fireEvent.pointerDown(slider(), { button: 0, clientX: 400, pointerId: 1 });
  expect(slider()).toHaveAttribute('aria-valuetext', 'Very high');
  expect(screen.getByRole('button', { name: 'Apply settings' })).toBeEnabled();
});

it('resets model and effort to the confirmed settings, and is unavailable when they match', async () => {
  const user = userEvent.setup();
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: async () => defaults,
        updateAgentSettings: vi.fn(),
      }),
    ),
  );
  await expandSettings();
  await openCard(user);
  const reset = within(card()).getByRole('button', { name: 'Reset to current settings' });
  expect(reset).toBeDisabled();
  await chooseOptionValue(user, model(), 'gpt-6-updated');
  await chooseEffort(user, 'Low');
  expect(chip()).toHaveTextContent('Updated model Low');
  expect(reset).toBeEnabled();
  await user.click(reset);
  expect(chip()).toHaveTextContent('GPT-6 High');
  expect(reset).toBeDisabled();
  expect(screen.queryByRole('button', { name: 'Apply settings' })).not.toBeInTheDocument();
});

it('closes the card with Escape or an outside press, and Escape in the model list closes only the list', async () => {
  const user = userEvent.setup();
  render(
    <>
      {draw(
        agentClient({
          readAgentModels: catalog,
          readAgentSettings: async () => defaults,
          updateAgentSettings: vi.fn(),
        }),
      )}
      <p>Conversation</p>
    </>,
  );
  await expandSettings();
  await openCard(user);
  expect(chip()).toHaveAttribute('aria-expanded', 'true');
  await user.click(model());
  expect(screen.getByRole('menu', { name: 'Model' })).toBeInTheDocument();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('menu', { name: 'Model' })).not.toBeInTheDocument();
  expect(card()).toBeInTheDocument();
  await user.keyboard('{Escape}');
  expect(
    screen.queryByRole('dialog', { name: 'Model and reasoning effort' }),
  ).not.toBeInTheDocument();
  expect(chip()).toHaveFocus();
  await user.click(chip());
  await user.click(within(card()).getByRole('slider'));
  expect(card()).toBeInTheDocument();
  await user.click(screen.getByText('Conversation'));
  expect(
    screen.queryByRole('dialog', { name: 'Model and reasoning effort' }),
  ).not.toBeInTheDocument();
});

it('offers a retry when the settings themselves cannot be read', async () => {
  const user = userEvent.setup();
  const read = vi
    .fn()
    .mockRejectedValueOnce(new Error('Settings unavailable'))
    .mockResolvedValue(defaults);
  render(
    draw(
      agentClient({
        readAgentModels: catalog,
        readAgentSettings: read,
        updateAgentSettings: vi.fn(),
      }),
    ),
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('Current settings could not be read.');
  expect(
    screen.queryByRole('button', { name: /^Model and reasoning effort/ }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Retry settings' }));
  await screen.findByRole('button', { name: /^Agent settings:/ }, { timeout: 2500 });
  await expandSettings();
  expect(
    await screen.findByRole('button', { name: /^Model and reasoning effort/ }),
  ).toHaveTextContent('GPT-6 High');
});
