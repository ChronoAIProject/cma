import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Composer } from '../Composer';
import { useState } from 'react';

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
  function Surface({ accepted }: { accepted?: string }) {
    const [content, setContent] = useState('Original message');
    return (
      <Composer
        {...props}
        draft={{ content, setContent, attachments: [], setAttachments: vi.fn() }}
        requestPending={accepted === undefined}
        acceptedMessage={accepted}
      />
    );
  }
  const { rerender } = render(<Surface />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  if (draft !== 'Original message') fireEvent.change(field, { target: { value: draft } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(field).toBeEnabled();
  rerender(<Surface accepted="Original message" />);
  expect(field).toHaveValue(expected);
  // The receipt cannot erase a later independently edited draft.
  fireEvent.change(field, { target: { value: 'Original message' } });
  rerender(<Surface accepted="Original message" />);
  expect(field).toHaveValue('Original message');
  expect(props.onSend).not.toHaveBeenCalled();
});
