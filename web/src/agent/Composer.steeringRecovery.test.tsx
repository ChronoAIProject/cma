import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';

it.each([false, true])(
  'recovered steering acknowledgement clears only the untouched submitted draft (new identical edit: %s)',
  async (edited) => {
    const send = vi.fn().mockResolvedValue(undefined);
    const draw = (acceptedSteeringMessage?: string, acceptedMessage = 'Earlier ordinary input') => (
      <Composer
        running
        status="working"
        queueWhileRunning
        steeringMode="interrupt"
        busy={false}
        inputAvailability={{ allowed: true }}
        acceptedMessage={acceptedMessage}
        acceptedSteeringMessage={acceptedSteeringMessage}
        onSend={send}
        onStop={vi.fn()}
      />
    );
    const { rerender } = render(draw());
    const field = screen.getByRole('textbox', { name: 'Message' });
    fireEvent.change(field, { target: { value: 'Steering draft' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Interrupt with message' }));
    });
    expect(field).toHaveValue('Steering draft');
    if (edited) {
      fireEvent.change(field, { target: { value: 'Changed while recovery waits' } });
      fireEvent.change(field, { target: { value: 'Steering draft' } });
    }
    rerender(draw('Steering draft'));
    expect(field).toHaveValue(edited ? 'Steering draft' : '');
    expect(send).toHaveBeenCalledOnce();
  },
);

it('tracks later ordinary acknowledgements separately from an accepted steering message', async () => {
  const send = vi.fn().mockResolvedValue(undefined);
  const draw = (acceptedMessage?: string) => (
    <Composer
      running={false}
      status="ready"
      busy={false}
      inputAvailability={{ allowed: true }}
      acceptedMessage={acceptedMessage}
      acceptedSteeringMessage="Previous steering input"
      onSend={send}
      onStop={vi.fn()}
    />
  );
  const { rerender } = render(draw());
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Next ordinary message' } });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  });
  rerender(draw('Next ordinary message'));
  expect(field).toHaveValue('');
});
