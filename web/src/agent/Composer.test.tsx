import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';

it('allows a v2 Starting message with the normal composer and retains its draft until acceptance', async () => {
  let accept: (() => void) | undefined;
  const onSend = vi.fn((_content: string, onAccepted: () => void) => {
    accept = onAccepted;
  });
  render(
    <Composer
      status="starting"
      queueWhileStarting
      running={false}
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={onSend}
      onStop={vi.fn()}
    />,
  );
  const user = userEvent.setup();
  const field = screen.getByRole('textbox', { name: 'Message' });
  expect(field).toBeEnabled();
  expect(field).toHaveAttribute('placeholder', 'Message Agent');
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  await user.type(field, 'Follow the setup');
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  expect(onSend).toHaveBeenCalledExactlyOnceWith('Follow the setup', expect.any(Function));
  expect(field).toHaveValue('Follow the setup');
  await act(async () => accept?.());
  expect(field).toHaveValue('');
});

it('keeps Starting input disabled unless durable queuing is explicitly enabled', async () => {
  const onSend = vi.fn();
  render(
    <Composer
      status="starting"
      running={false}
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={onSend}
      onStop={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  expect(field).toBeDisabled();
  expect(field).toHaveAttribute('placeholder', 'Message Agent');
  expect(screen.queryByText('Agent is starting.')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
  expect(onSend).not.toHaveBeenCalled();
});

function editableComposer() {
  const onSend = vi.fn();
  render(
    <Composer
      status="ready"
      running={false}
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={onSend}
      onStop={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: '  draft in composition  ' } });
  return { field, onSend };
}

it('preserves the draft when Enter confirms composition, including missing native composition flags', async () => {
  const { field, onSend } = editableComposer();
  fireEvent.compositionStart(field);
  expect(fireEvent.keyDown(field, { key: 'Enter', isComposing: false })).toBe(true);
  fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
  expect(onSend).not.toHaveBeenCalled();
  expect(field).toHaveValue('  draft in composition  ');
  fireEvent.compositionEnd(field);
  expect(onSend).not.toHaveBeenCalled();
  await act(async () => {
    fireEvent.keyDown(field, { key: 'Enter' });
  });
  expect(onSend).toHaveBeenCalledExactlyOnceWith('draft in composition', expect.any(Function));
});

it.each([{ isComposing: true }, { isComposing: false, keyCode: 229 }])(
  'does not consume IME Enter with native flags %j',
  (flags) => {
    const { field, onSend } = editableComposer();
    fireEvent.compositionStart(field);
    fireEvent.compositionEnd(field);
    expect(fireEvent.keyDown(field, { key: 'Enter', ...flags })).toBe(true);
    expect(onSend).not.toHaveBeenCalled();
    expect(field).toHaveValue('  draft in composition  ');
  },
);

it('retains Shift+Enter newlines and sends only on ordinary Enter', async () => {
  const { field, onSend } = editableComposer();
  const actor = userEvent.setup();
  await actor.type(field, '{Shift>}{Enter}{/Shift}second line');
  expect(field).toHaveValue('  draft in composition  \nsecond line');
  expect(onSend).not.toHaveBeenCalled();
  await actor.keyboard('{Enter}');
  expect(onSend).toHaveBeenCalledExactlyOnceWith(
    'draft in composition  \nsecond line',
    expect.any(Function),
  );
});

it('retains an editable draft and reports saving while holding both Enter and Send', async () => {
  const onSend = vi.fn();
  const actor = userEvent.setup();
  const { rerender } = render(
    <Composer
      status="ready"
      running={false}
      busy={false}
      inputAvailability={{
        allowed: false,
        notice: 'Saving progress. Messages will be available when saving finishes.',
        savingStartedAt: '2026-09-08T00:00:00Z',
      }}
      onSend={onSend}
      onStop={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  await actor.type(field, '  Keep this draft  {Enter}');
  expect(field).toBeEnabled();
  expect(field).toHaveValue('  Keep this draft  ');
  expect(field).toHaveAccessibleDescription(/Saving progress/);
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(screen.getByRole('status').querySelector('time')).toHaveAttribute(
    'dateTime',
    '2026-09-08T00:00:00Z',
  );
  expect(onSend).not.toHaveBeenCalled();
  rerender(
    <Composer
      status="ready"
      running={false}
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={onSend}
      onStop={vi.fn()}
    />,
  );
  expect(field).toHaveValue('  Keep this draft  ');
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(onSend).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'Send message' }));
  expect(onSend).toHaveBeenCalledWith('Keep this draft', expect.any(Function));
});

it('silently awaits admission and preserves Stop for actual ongoing work during saving', async () => {
  const onStop = vi.fn();
  const actor = userEvent.setup();
  const { rerender } = render(
    <Composer
      status="ready"
      running={false}
      starting
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={vi.fn()}
      onStop={onStop}
    />,
  );
  expect(screen.queryByText('Starting.', { exact: true })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  rerender(
    <Composer
      status="working"
      running
      busy={false}
      inputAvailability={{ allowed: false, notice: 'Saving progress.' }}
      onSend={vi.fn()}
      onStop={onStop}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'Stop response' }));
  expect(onStop).toHaveBeenCalledOnce();
});

it('renders the paused notice verbatim and sends through the normal composer flow', async () => {
  const onSend = vi.fn();
  const actor = userEvent.setup();
  render(
    <Composer
      status="paused"
      pausedNotice="This Agent's Sandbox is paused."
      running={false}
      inputAvailability={{ allowed: true }}
      busy={false}
      onSend={onSend}
      onStop={vi.fn()}
    />,
  );
  expect(screen.getByText("This Agent's Sandbox is paused.")).toBeInTheDocument();
  const composer = screen.getByRole('textbox', { name: 'Message' });
  expect(composer).toHaveAttribute('placeholder', 'Send a message to continue.');
  expect(composer).toHaveFocus();
  await actor.type(composer, 'Revive and continue.{Enter}');
  expect(onSend).toHaveBeenCalledWith('Revive and continue.', expect.any(Function));
});

it('keeps a failed admission draft byte-for-byte and clears only the accepted draft', async () => {
  const actor = userEvent.setup();
  let accept: (() => void) | undefined;
  const onSend = vi.fn((_content: string, onAccepted: () => void) => {
    accept = onAccepted;
  });
  render(
    <Composer
      status="ready"
      running={false}
      inputAvailability={{ allowed: true }}
      busy={false}
      onSend={onSend}
      onStop={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  await actor.type(field, '  Keep this draft.  ');
  await actor.click(screen.getByRole('button', { name: 'Send message' }));
  expect(field).toHaveValue('  Keep this draft.  ');
  act(() => accept?.());
  expect(field).toHaveValue('');
  await actor.type(field, 'First draft');
  await actor.click(screen.getByRole('button', { name: 'Send message' }));
  await actor.type(field, ' edited');
  act(() => accept?.());
  expect(field).toHaveValue('First draft edited');
});

it('fits new content after each change without accumulating the previous height', () => {
  render(
    <Composer
      status="ready"
      running={false}
      inputAvailability={{ allowed: true }}
      busy={false}
      onSend={vi.fn()}
      onStop={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  Object.defineProperty(field, 'scrollHeight', { configurable: true, value: 140 });
  fireEvent.change(field, { target: { value: 'Multiple lines' } });
  expect(field.style.height).toBe('140px');
  Object.defineProperty(field, 'scrollHeight', { configurable: true, value: 24 });
  fireEvent.change(field, { target: { value: '' } });
  expect(field.style.height).toBe('24px');
});

it('replaces Send with Stop while an Agent response is running', async () => {
  const onStop = vi.fn();
  const actor = userEvent.setup();
  const { rerender } = render(
    <Composer
      status="working"
      running
      inputAvailability={{ allowed: true }}
      busy={false}
      onSend={vi.fn()}
      onStop={onStop}
    />,
  );
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Stop response' }));
  expect(onStop).toHaveBeenCalledOnce();

  rerender(
    <Composer
      status="working"
      running
      inputAvailability={{ allowed: true }}
      busy
      onSend={vi.fn()}
      onStop={onStop}
    />,
  );
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeDisabled();
  await actor.click(screen.getByRole('button', { name: 'Stop response' }));
  expect(onStop).toHaveBeenCalledOnce();
});

it('clears the running composer state when the recorded turn completes', () => {
  const draw = (running: boolean) => (
    <Composer
      status={running ? 'working' : 'ready'}
      running={running}
      queueWhileRunning
      inputAvailability={{ allowed: true }}
      busy={false}
      onSend={vi.fn()}
      onStop={vi.fn()}
    />
  );
  const { rerender } = render(draw(true));
  expect(screen.getByPlaceholderText('Add a follow-up while the Agent works.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeInTheDocument();
  rerender(draw(false));
  expect(screen.getByPlaceholderText('Message Agent')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeInTheDocument();
});

it('keeps every control inside the box and offers steering only while a turn runs', async () => {
  const actor = userEvent.setup();
  const onSteeringModeChange = vi.fn();
  const draw = (running: boolean) => (
    <Composer
      status={running ? 'working' : 'ready'}
      running={running}
      queueWhileRunning
      showUsage
      steeringMode="queue"
      onSteeringModeChange={onSteeringModeChange}
      busy={false}
      inputAvailability={{ allowed: true }}
      uploadAttachment={vi.fn()}
      onSend={vi.fn()}
      onStop={vi.fn()}
    />
  );
  const { rerender } = render(draw(false));
  const box = screen.getByRole('textbox', { name: 'Message' }).parentElement!;
  expect(box).toHaveClass('agent-composer__box');
  for (const name of ['Attach files', 'Context usage', 'Send message'])
    expect(box).toContainElement(screen.getByRole('button', { name }));
  expect(screen.queryByRole('button', { name: /Steering mode/ })).not.toBeInTheDocument();
  rerender(draw(true));
  const steering = screen.getByRole('button', { name: 'Steering mode: Queue' });
  expect(box).toContainElement(steering);
  expect(box).toContainElement(screen.getByRole('button', { name: 'Queue message' }));
  expect(box).toContainElement(screen.getByRole('button', { name: 'Stop response' }));
  await actor.click(steering);
  await actor.click(screen.getByRole('menuitemradio', { name: /Interrupt/ }));
  expect(onSteeringModeChange).toHaveBeenCalledExactlyOnceWith('interrupt');
  rerender(draw(false));
  expect(screen.queryByRole('button', { name: /Steering mode/ })).not.toBeInTheDocument();
});

it('does not offer steering to a tab that cannot queue behind a running turn', () => {
  render(
    <Composer
      status="working"
      running
      steeringMode="queue"
      onSteeringModeChange={vi.fn()}
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={vi.fn()}
      onStop={vi.fn()}
    />,
  );
  expect(screen.queryByRole('button', { name: /Steering mode/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Queue message' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
});
