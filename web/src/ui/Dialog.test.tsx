import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
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

describe('the dialog shell', () => {
  it('offers three widths and no others', () => {
    // §7. A width chosen per surface is how two dialogs that ask the same kind
    // of question end up different sizes.
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Dialog.css'), 'utf8');
    const widths = [...css.matchAll(/\.dialog--(\w+)\s*\{\s*max-width: (\d+)px/g)].map((match) => [
      match[1],
      match[2],
    ]);
    expect(widths).toEqual([
      ['narrow', '520'],
      ['medium', '700'],
      ['wide', '1000'],
    ]);
    // And no other rule sets a dialog's width.
    expect(css.match(/max-width: \d+px/g)).toHaveLength(3);
  });

  it('is a plate: it takes the card impression and never moves', () => {
    // §3 lists a plate among the things that carry the 5px impression and
    // among the things that do not press in on hover.
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Dialog.css'), 'utf8');
    expect(css).toMatch(/\.dialog\s*\{[^}]*box-shadow: var\(--press-card-rest\)/);
    expect(css).not.toMatch(/\.dialog[^_{]*:hover/);
    expect(css).not.toMatch(/\.dialog[\s\S]*?transform:/);
  });

  it('is the only shell in the tree', () => {
    // The parts that are easy to get wrong — the focus trap, the return of
    // focus, Escape, the scrim click — are the parts nobody notices are
    // missing. There were two implementations and one trapped nothing, so
    // this asserts that only one declares the shell.
    const shells = readdirSync(resolve(process.cwd(), 'src'), { recursive: true })
      .filter((name) => typeof name === 'string' && name.endsWith('.css'))
      .map((name) => [name, readFileSync(resolve(process.cwd(), 'src', name as string), 'utf8')])
      .filter(([, css]) => (css as string).includes('.dialog__scrim'))
      .map(([name]) => name);
    expect(shells).toEqual(['ui/Dialog.css']);
  });
});
