import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Modal } from './Modal';

function open(onClose = vi.fn()) {
  render(
    <div>
      <button type="button">behind</button>
      <Modal
        open
        title="Environment profiles"
        onClose={onClose}
        footer={<button type="button">Save</button>}
      >
        <button type="button">inside</button>
      </Modal>
    </div>,
  );
  return onClose;
}

describe('the dialog', () => {
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

  it.each([false, true])(
    'uses the preferred safe action only when enabled (disabled=%s)',
    (disabled) => {
      render(
        <Modal
          open
          title="Keep your draft?"
          onClose={vi.fn()}
          footer={
            <button data-initial-focus="true" disabled={disabled}>
              Keep editing
            </button>
          }
        />,
      );
      expect(
        screen.getByRole('button', { name: disabled ? 'Close' : 'Keep editing' }),
      ).toHaveFocus();
    },
  );

  it('closes on Escape', async () => {
    const user = userEvent.setup();
    const onClose = open();

    await user.keyboard('{Escape}');

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('holds an unresolved command dialog until its caller makes it dismissible', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Modal open dismissible={false} title="Unresolved command" onClose={onClose} />);

    expect(screen.getByRole('button', { name: 'Close' })).toBeDisabled();
    await user.keyboard('{Escape}');
    await user.click(document.querySelector('.modal__mask') as HTMLElement);

    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog', { name: 'Unresolved command' })).toBeInTheDocument();
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

    // The mask is the only part of the overlay that dismisses.
    await user.click(document.querySelector('.modal__mask') as HTMLElement);
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

  it('keeps focus inside a controlled field when its owner re-renders', async () => {
    const user = userEvent.setup();

    function ControlledDialog() {
      const [value, setValue] = useState('');
      return (
        <Modal open title="Edit" onClose={() => undefined}>
          <label>
            Title
            <input value={value} onChange={(event) => setValue(event.target.value)} />
          </label>
        </Modal>
      );
    }

    render(<ControlledDialog />);
    const title = screen.getByLabelText('Title');
    await user.click(title);
    await user.type(title, 'One retained title');

    expect(title).toHaveFocus();
    expect(title).toHaveValue('One retained title');
  });

  it('returns focus to whatever opened it', () => {
    const opener = document.createElement('button');
    opener.textContent = 'opener';
    document.body.append(opener);
    opener.focus();

    const { unmount } = render(
      <Modal open title="Sized" onClose={vi.fn()}>
        <button type="button">inside</button>
      </Modal>,
    );
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();

    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });

  it('renders without a footer when a surface has no actions', () => {
    render(
      <Modal open title="Read only" onClose={vi.fn()}>
        <p>Nothing to do here.</p>
      </Modal>,
    );
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
  });
});

describe('the dialog shell', () => {
  it('keeps every dialog centered inside the same 20% viewport margins', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Modal.css'), 'utf8');
    expect(css).toMatch(/\.modal\s*\{[^}]*align-items: center/);
    expect(css).toMatch(/\.modal\s*\{[^}]*justify-content: center/);
    expect(css).toMatch(/\.modal\s*\{[^}]*padding: 20vh 20vw/);
    expect(css).toMatch(/\.modal\s*\{[^}]*padding-block: 20dvh/);
    expect(css).toMatch(/\.modal__card\s*\{[^}]*box-sizing: border-box/);
    expect(css).toMatch(/\.modal__card\s*\{[^}]*width: min\(700px, 60vw\)/);
    expect(css).toMatch(/\.modal__card\s*\{[^}]*max-width: 60vw/);
    expect(css).toMatch(/\.modal__card\s*\{[^}]*max-height: 60dvh/);
    expect(css).toMatch(/\.modal__card\s*\{[^}]*border-radius: 16px/);
    expect(css).toMatch(/\.modal__body\s*\{[^}]*overflow: auto/);
    expect(css).not.toMatch(/\.modal--wide(?:st)?\s*\{/);
  });

  it('gives a phone the full width less a 16px gutter', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Modal.css'), 'utf8');
    const phone = css.slice(css.indexOf('@media (max-width: 480px) {'));
    expect(phone).toMatch(/\.modal\s*\{[^}]*padding-inline: 16px/);
    expect(phone).toMatch(/\.modal__card\s*\{[^}]*width: 100%[^}]*max-width: 100%/);
  });

  it('lets a phone dialog use the full height less a 16px gutter', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Modal.css'), 'utf8');
    const phone = css.slice(css.indexOf('@media (max-width: 480px) {'));
    expect(phone).toMatch(/\.modal\s*\{[^}]*padding-block: 16px/);
    expect(phone).toMatch(/\.modal__card\s*\{[^}]*max-height: calc\(100dvh - 32px\)/);
  });

  it.each([undefined, 'modal--wide', 'modal--widest'])(
    'uses the shared card and scrolling content for legacy placement %s',
    (placement) => {
      render(
        <Modal open title="Shared dialog" placement={placement} onClose={vi.fn()}>
          <p>Long content remains within the shared body.</p>
        </Modal>,
      );
      const dialog = screen.getByRole('dialog', { name: 'Shared dialog' });
      expect(dialog).toHaveClass('modal__card');
      expect(
        screen.getByText('Long content remains within the shared body.').parentElement,
      ).toHaveClass('modal__body');
    },
  );

  it('portals out of the page, and stops it scrolling underneath', () => {
    // An ancestor that establishes a stacking context leaves sticky page
    // chrome floating above the mask, and no z-index here can reach past it.
    const host = document.createElement('div');
    document.body.append(host);
    const { unmount } = render(
      <Modal open title="Sized" onClose={vi.fn()}>
        <p>body</p>
      </Modal>,
      { container: host },
    );
    // Out of whatever rendered it, and directly on the body.
    expect(screen.getByRole('dialog').closest('.modal')?.parentElement).toBe(document.body);
    expect(host.contains(screen.getByRole('dialog'))).toBe(false);
    expect(document.body.style.overflow).toBe('hidden');

    unmount();
    host.remove();
    // And gives it back, or the page stays frozen after the dialog leaves.
    expect(document.body.style.overflow).not.toBe('hidden');
  });

  it('renders nothing at all when closed', () => {
    // Not merely hidden: a closed dialog that is still mounted keeps its
    // focus trap armed and steals focus from the page behind it.
    render(
      <Modal open={false} title="Sized" onClose={vi.fn()}>
        <button type="button">inside</button>
      </Modal>,
    );
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.queryByRole('button', { name: 'inside' })).toBeNull();
  });

  it('is the only shell in the tree', () => {
    // The parts that are easy to get wrong — the focus trap, the return of
    // focus, Escape, the mask click — are the parts nobody notices are
    // missing. There were two implementations once and one trapped nothing,
    // so this asserts that only one declares the shell.
    const shells = readdirSync(resolve(process.cwd(), 'src'), { recursive: true })
      .filter((name) => typeof name === 'string' && name.endsWith('.css'))
      .map((name) => [name, readFileSync(resolve(process.cwd(), 'src', name as string), 'utf8')])
      .filter(([, css]) => (css as string).includes('.modal__mask'))
      .map(([name]) => name);
    expect(shells).toEqual(['ui/Modal.css']);
  });
});

it.each([undefined, 'sections'] as const)(
  'retains a %s editor without a closed focus trap or scroll lock',
  async (layout) => {
    const user = userEvent.setup();
    const renderDialog = (open: boolean) => (
      <Modal open={open} keepMounted layout={layout} title="Retained editor" onClose={vi.fn()}>
        <input aria-label="Draft" defaultValue="" />
      </Modal>
    );
    const { rerender, unmount } = render(renderDialog(true));
    const input = screen.getByRole('textbox', { name: 'Draft' });
    await user.type(input, 'Keep this uncertain edit');
    await act(async () => rerender(renderDialog(false)));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.body.style.overflow).not.toBe('hidden');
    expect(input.closest('.modal')).toHaveAttribute('inert');
    await act(async () => rerender(renderDialog(true)));
    expect(screen.getByRole('textbox', { name: 'Draft' })).toBe(input);
    expect(input).toHaveValue('Keep this uncertain edit');
    unmount();
    expect(document.body.style.overflow).not.toBe('hidden');
  },
);
