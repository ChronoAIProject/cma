import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';
import type { ComponentProps } from 'react';

const messages = [
  {
    name: 'Introduce Truretruing',
    description: 'Introduce the project and what you can do with it.',
    text: 'Read the project introduction and explain its purpose.',
  },
] as const;
const defaults: ComponentProps<typeof Composer> = {
  status: 'ready',
  busy: false,
  running: false,
  inputAvailability: { allowed: true },
  onSend: vi.fn(),
  onStop: vi.fn(),
  quickMessages: messages,
};

it('sends the exact shortcut once through normal admission while preserving a typed draft', async () => {
  const actor = userEvent.setup();
  let finish!: () => void;
  const onSend = vi.fn((_text: string, accepted: () => void) => {
    accepted();
    return new Promise<void>((resolve) => {
      finish = resolve;
    });
  });
  render(<Composer {...defaults} onSend={onSend} />);
  const draft = screen.getByRole('textbox', { name: 'Message' });
  await actor.type(draft, '  My unfinished draft  ');
  const button = screen.getByRole('button', { name: messages[0].name });
  expect(onSend).not.toHaveBeenCalled();
  await actor.dblClick(button);
  expect(onSend).toHaveBeenCalledExactlyOnceWith(messages[0].text, expect.any(Function));
  expect(draft).toHaveValue('  My unfinished draft  ');
  expect(button).toBeDisabled();
  await act(async () => finish());
  expect(button).toBeEnabled();
  expect(draft).toHaveValue('  My unfinished draft  ');
});

it.each([
  { busy: true },
  { running: true },
  { starting: true },
  { status: 'waiting_approval' as const },
  { status: 'deleted' as const },
  { inputAvailability: { allowed: false } },
])('holds shortcuts behind the same conversation guard: %j', async (override) => {
  const onSend = vi.fn();
  render(<Composer {...defaults} {...override} onSend={onSend} />);
  const button = screen.getByRole('button', { name: messages[0].name });
  expect(button).toBeDisabled();
  await userEvent.setup().click(button);
  expect(onSend).not.toHaveBeenCalled();
});

it('shows descriptions on hover and focus, dismisses with Escape and never sends from a preview', async () => {
  const onSend = vi.fn();
  const actor = userEvent.setup();
  render(<Composer {...defaults} onSend={onSend} />);
  const button = screen.getByRole('button', { name: messages[0].name });
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  await actor.hover(button);
  expect(screen.getByRole('tooltip')).toHaveTextContent(messages[0].description);
  await actor.unhover(button);
  await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  fireEvent.focus(button);
  expect(screen.getByRole('tooltip')).toBeVisible();
  expect(button).toHaveAccessibleDescription(messages[0].description);
  fireEvent.keyDown(button, { key: 'Escape' });
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  expect(onSend).not.toHaveBeenCalled();
});

it('shows only the latest description when focus and hover are on different shortcuts', async () => {
  const actor = userEvent.setup();
  render(
    <Composer
      {...defaults}
      quickMessages={[
        ...messages,
        { name: 'Contribute', description: 'Ready artifacts', text: 'Collect artifacts' },
      ]}
    />,
  );
  fireEvent.focus(screen.getByRole('button', { name: messages[0].name }));
  await actor.hover(screen.getByRole('button', { name: 'Contribute' }));
  expect(screen.getAllByRole('tooltip')).toHaveLength(1);
  expect(screen.getByRole('tooltip')).toHaveTextContent('Ready artifacts');
});
