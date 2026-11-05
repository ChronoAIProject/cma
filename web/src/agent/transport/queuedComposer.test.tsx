import { fireEvent, render, screen, act } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Composer } from '../Composer';

const props = {
  status: 'working' as const,
  running: true,
  queueWhileRunning: true,
  inputAvailability: { allowed: true },
  busy: false,
  onStop: vi.fn(),
};

it.each(['click', 'enter'])(
  'queues ordinary input with %s and restores the single Stop button after acceptance',
  async (method) => {
    const onSend = vi.fn(async (_text, accept) => accept());
    render(<Composer {...props} onSend={onSend} />);
    const field = screen.getByRole('textbox', { name: 'Message' });
    fireEvent.change(field, { target: { value: 'Do this after the current response' } });
    expect(screen.getByRole('button', { name: 'Queue message' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
    await act(async () => {
      if (method === 'click')
        fireEvent.click(screen.getByRole('button', { name: 'Queue message' }));
      else fireEvent.keyDown(field, { key: 'Enter' });
    });
    expect(onSend).toHaveBeenCalledOnce();
    expect(onSend.mock.calls[0]![0]).toBe('Do this after the current response');
    expect(field).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
    expect(props.onStop).not.toHaveBeenCalled();
  },
);

it('retains an editable draft and blocks queue admission while a prior request is uncertain', () => {
  const onSend = vi.fn();
  render(<Composer {...props} onSend={onSend} requestPending />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep this draft' } });
  fireEvent.keyDown(field, { key: 'Enter' });
  expect(screen.getByRole('button', { name: 'Queue message' })).toBeDisabled();
  expect(field).toBeEnabled();
  expect(field).toHaveValue('Keep this draft');
  expect(onSend).not.toHaveBeenCalled();
});
