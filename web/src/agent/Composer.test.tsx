import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';

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
