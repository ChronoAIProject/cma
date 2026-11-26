import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';

const props = () => ({
  status: 'working' as const,
  running: true,
  busy: false,
  inputAvailability: { allowed: true },
  onSend: vi.fn(),
  onStop: vi.fn(),
});

it.each([{ controlsUnavailable: true }, { busy: true }, { stopping: true }])(
  'does not advertise or submit an unavailable Stop: %j',
  (state) => {
    const current = props();
    render(<Composer {...current} {...state} />);
    expect(screen.getByRole('button', { name: /Stop.*response/ })).toBeDisabled();
    expect(screen.queryByText('Esc to stop')).not.toBeInTheDocument();
    expect(screen.queryByText('Running Sandbox commands may continue.')).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message' }), { key: 'Escape' });
    expect(current.onStop).not.toHaveBeenCalled();
  },
);

it.each(['Escape', 'button'] as const)(
  'deduplicates %s and click while Stop is pending',
  async (first) => {
    let finish!: () => void;
    const onStop = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    render(<Composer {...props()} onStop={onStop} />);
    const field = screen.getByRole('textbox', { name: 'Message' });
    const button = screen.getByRole('button', { name: 'Stop response' });
    if (first === 'Escape') fireEvent.keyDown(field, { key: 'Escape' });
    else fireEvent.click(button);
    fireEvent.keyDown(field, { key: 'Escape' });
    fireEvent.click(button);
    expect(onStop).toHaveBeenCalledOnce();
    expect(button).toBeDisabled();
    expect(screen.queryByText('Esc to stop')).not.toBeInTheDocument();
    await act(async () => finish());
  },
);

it('advertises Escape only in the message field and yields to commands and IME', () => {
  const current = props();
  render(
    <>
      <button>Desktop</button>
      <Composer {...current} queueWhileRunning availableCommands={[]} />
    </>,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  expect(screen.getByText('Esc to stop')).toBeVisible();
  act(() => screen.getByRole('button', { name: 'Desktop' }).focus());
  expect(screen.queryByText('Esc to stop')).not.toBeInTheDocument();
  const scope = screen.getByText('Running Sandbox commands may continue.');
  expect(scope).toBeVisible();
  expect(scope).not.toHaveClass('sr-only');
  expect(screen.getByRole('button', { name: 'Stop response' })).toHaveAccessibleDescription(
    'Running Sandbox commands may continue.',
  );
  fireEvent.keyDown(screen.getByRole('button', { name: 'Desktop' }), { key: 'Escape' });
  act(() => field.focus());
  fireEvent.compositionStart(field);
  expect(screen.queryByText('Esc to stop')).not.toBeInTheDocument();
  fireEvent.keyDown(field, { key: 'Escape', isComposing: true });
  fireEvent.compositionEnd(field);
  fireEvent.change(field, { target: { value: '/' } });
  fireEvent.keyDown(field, { key: 'Escape' });
  expect(current.onStop).not.toHaveBeenCalled();
  fireEvent.change(field, { target: { value: '' } });
  expect(screen.getByText('Esc to stop')).toBeVisible();
  fireEvent.keyDown(field, { key: 'Escape' });
  expect(current.onStop).toHaveBeenCalledOnce();
});

it.each([
  ['checking', 'Checking response controls…'],
  ['checking-access', 'Checking Agent access…'],
  ['unbound', 'Stop is unavailable for this response.'],
  ['denied', 'Only the owner can stop this response.'],
] as const)(
  'updates button, hint and Escape together through %s and completion',
  async (availability, notice) => {
    const current = props();
    const view = render(<Composer {...current} />);
    const field = screen.getByRole('textbox', { name: 'Message' });
    view.rerender(<Composer {...current} responseControls={availability} />);
    const stop = screen.getByRole('button', { name: 'Stop response' });
    expect(stop).toBeDisabled();
    expect(stop).toHaveAccessibleDescription(notice);
    expect(screen.queryByText('Esc to stop')).not.toBeInTheDocument();
    fireEvent.keyDown(field, { key: 'Escape' });
    expect(current.onStop).not.toHaveBeenCalled();
    view.rerender(<Composer {...current} responseControls="available" />);
    expect(stop).toBeEnabled();
    expect(screen.getByText('Esc to stop')).toBeVisible();
    expect(stop).toHaveAccessibleDescription('Esc to stop Running Sandbox commands may continue.');
    expect(stop).toHaveAttribute(
      'title',
      'Stop response (Esc). Running Sandbox commands may continue.',
    );
    await act(async () => fireEvent.keyDown(field, { key: 'Escape' }));
    expect(current.onStop).toHaveBeenCalledOnce();
    view.rerender(<Composer {...current} running={false} status="ready" />);
    expect(screen.queryByText('Esc to stop')).not.toBeInTheDocument();
    expect(screen.queryByText('Running Sandbox commands may continue.')).not.toBeInTheDocument();
    fireEvent.keyDown(field, { key: 'Escape' });
    expect(current.onStop).toHaveBeenCalledOnce();
  },
);

it('does not consume Escape from another Agent, dialog or computer control', () => {
  const first = props();
  const second = props();
  const draw = (current: ReturnType<typeof props>, key: string) => (
    <>
      <Composer key={key} {...current} />
      <div role="dialog">
        <button>Close</button>
      </div>
      <div contentEditable suppressContentEditableWarning role="textbox" aria-label="Computer" />
    </>
  );
  const view = render(draw(first, 'first-agent'));
  for (const target of [
    screen.getByRole('button', { name: 'Close' }),
    screen.getByRole('textbox', { name: 'Computer' }),
  ]) {
    act(() => target.focus());
    fireEvent.keyDown(target, { key: 'Escape' });
    expect(screen.queryByText('Esc to stop')).not.toBeInTheDocument();
  }
  expect(first.onStop).not.toHaveBeenCalled();
  expect(second.onStop).not.toHaveBeenCalled();
  view.rerender(draw(second, 'second-agent'));
  const field = screen.getByRole('textbox', { name: 'Message' });
  act(() => field.focus());
  fireEvent.keyDown(field, { key: 'Escape' });
  expect(second.onStop).toHaveBeenCalledOnce();
  expect(first.onStop).not.toHaveBeenCalled();
});
