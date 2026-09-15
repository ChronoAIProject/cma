import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Composer } from '../Composer';

it.each([
  ['Original message', ''],
  ['Edited draft', 'Edited draft'],
])('settles only the matching draft after explicit recovery: %s', (draft, expected) => {
  const props = {
    status: 'ready' as const,
    running: false,
    busy: false,
    inputAvailability: { allowed: true },
    onSend: vi.fn(),
    onStop: vi.fn(),
  };
  const { rerender } = render(<Composer {...props} requestPending />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: draft } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(field).toBeEnabled();
  rerender(<Composer {...props} acceptedMessage="Original message" />);
  expect(field).toHaveValue(expected);
  // The receipt cannot erase a later independently edited draft.
  fireEvent.change(field, { target: { value: 'Original message' } });
  rerender(<Composer {...props} acceptedMessage="Original message" />);
  expect(field).toHaveValue('Original message');
  expect(props.onSend).not.toHaveBeenCalled();
});
