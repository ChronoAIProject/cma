import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';

it('renders the paused notice verbatim and sends through the normal composer flow', async () => {
  const onSend = vi.fn();
  const actor = userEvent.setup();
  render(
    <Composer
      status="paused"
      pausedNotice="This Agent's Sandbox is paused."
      running={false}
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
  render(<Composer status="ready" running={false} busy={false} onSend={onSend} onStop={vi.fn()} />);
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
    <Composer status="ready" running={false} busy={false} onSend={vi.fn()} onStop={vi.fn()} />,
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
    <Composer status="working" running busy={false} onSend={vi.fn()} onStop={onStop} />,
  );
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Stop response' }));
  expect(onStop).toHaveBeenCalledOnce();

  rerender(<Composer status="working" running busy onSend={vi.fn()} onStop={onStop} />);
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeDisabled();
  await actor.click(screen.getByRole('button', { name: 'Stop response' }));
  expect(onStop).toHaveBeenCalledOnce();
});
