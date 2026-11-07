import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';

it.each(['mount', 'ready'] as const)(
  'preserves computer-tab focus when the composer becomes usable on %s',
  (stage) => {
    const props = {
      running: false,
      busy: false,
      inputAvailability: { allowed: true },
      onSend: vi.fn(),
      onStop: vi.fn(),
    };
    const view = render(
      <>
        <button role="tab">Screen</button>
        {stage === 'ready' && <Composer {...props} status="starting" />}
      </>,
    );
    const tab = screen.getByRole('tab', { name: 'Screen' });
    tab.focus();
    view.rerender(
      <>
        <button role="tab">Screen</button>
        <Composer {...props} status="ready" />
      </>,
    );
    expect(tab).toHaveFocus();
    expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();
  },
);

it('focuses a newly usable composer when the owner has not chosen another control', () => {
  const props = {
    running: false,
    busy: false,
    inputAvailability: { allowed: true },
    onSend: vi.fn(),
    onStop: vi.fn(),
  };
  const view = render(<Composer {...props} status="starting" />);
  expect(document.body).toHaveFocus();
  view.rerender(<Composer {...props} status="ready" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveFocus();
});
