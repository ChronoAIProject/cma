import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Dialog } from './Dialog';

function open(onClose = vi.fn()) {
  render(
    <div>
      <button type="button">behind</button>
      <Dialog
        title="Environment profiles"
        onClose={onClose}
        footer={<button type="button">Save</button>}
      >
        <button type="button">inside</button>
      </Dialog>
    </div>,
  );
  return onClose;
}

describe('Dialog', () => {
  it('announces itself as a modal labelled by its heading', () => {
    open();
    const dialog = screen.getByRole('dialog', { name: 'Environment profiles' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
  });

  it('moves focus inside on open', () => {
    open();
    // Otherwise a keyboard user is left on the page behind, with no signal
    // anything happened.
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
  });

  it('closes on Escape', async () => {
    const user = userEvent.setup();
    const onClose = open();

    await user.keyboard('{Escape}');

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('closes on the close control', async () => {
    const user = userEvent.setup();
    const onClose = open();

    await user.click(screen.getByRole('button', { name: 'Close' }));

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('closes on a scrim click but not on a click inside', async () => {
    const user = userEvent.setup();
    const onClose = open();

    await user.click(screen.getByRole('button', { name: 'inside' }));
    expect(onClose).not.toHaveBeenCalled();

    // The scrim is the only part of the overlay that dismisses.
    await user.click(screen.getByRole('dialog').parentElement as HTMLElement);
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('traps Tab so the page behind stays unreachable', async () => {
    const user = userEvent.setup();
    open();

    // Forward off the last control wraps to the first, rather than landing on
    // the button behind the scrim.
    await user.tab();
    await user.tab();
    await user.tab();
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();

    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name: 'Save' })).toHaveFocus();
  });

  it('returns focus to whatever opened it', () => {
    const opener = document.createElement('button');
    opener.textContent = 'opener';
    document.body.append(opener);
    opener.focus();

    const { unmount } = render(
      <Dialog title="Sized" onClose={vi.fn()}>
        <button type="button">inside</button>
      </Dialog>,
    );
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();

    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });

  it('renders without a footer when a surface has no actions', () => {
    render(
      <Dialog title="Read only" onClose={vi.fn()}>
        <p>Nothing to do here.</p>
      </Dialog>,
    );
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
  });
});
