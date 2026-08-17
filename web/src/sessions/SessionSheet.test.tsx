import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { SessionDetails } from './SessionDetails';
import { SessionHeader } from './SessionHeader';
import { SessionActions } from './SessionActions';
import { abilitiesFor } from './availability';
import type { Session, SessionLifecycle } from './types';

/**
 * The session header and the details sheet — docs/Web_Style.md §12 and §11.
 */

function session(lifecycle: SessionLifecycle, extra: Partial<Session> = {}): Session {
  return {
    id: 'ses_test',
    kernel: 'codex',
    lifecycle,
    createdAt: Date.parse('2026-08-18T08:53:00Z'),
    ...extra,
  };
}

function renderActions(lifecycle: SessionLifecycle, extra: Partial<Session> = {}) {
  return render(
    <SessionActions
      session={session(lifecycle, extra)}
      onReconnect={vi.fn()}
      onRevive={vi.fn()}
      onStop={vi.fn()}
      onDelete={vi.fn()}
    />,
  );
}

describe('the session header', () => {
  it('carries the lifecycle controls, visible without opening anything', () => {
    // §12: a session's lifecycle controls live in the session header, at the
    // head of the terminal they act on.
    const { container } = render(
      <SessionHeader
        session={session('running')}
        actions={
          <SessionActions
            session={session('running')}
            onReconnect={vi.fn()}
            onRevive={vi.fn()}
            onStop={vi.fn()}
            onDelete={vi.fn()}
          />
        }
      />,
    );
    expect(container.querySelector('.sessionheader .actions')).not.toBeNull();
    expect(screen.getByRole('button', { name: /^reconnect$/i })).toBeEnabled();
  });

  it.each([
    ['provisioning', 'This session is still starting.'],
    ['rotating', 'This session is moving to an updated runtime.'],
    ['idle', 'This session is stopped. Revive it to carry on.'],
  ] as const)('states why %s disables what it disables', (lifecycle, reason) => {
    // §7 and §11: every disabled control states why, on the surface, never
    // only in a tooltip. `abilitiesFor` disables for exactly one cause at a
    // time, so one sentence covers every control it disabled.
    renderActions(lifecycle);
    const stated = screen.getByText(reason);
    expect(stated).toBeInTheDocument();

    for (const button of screen.getAllByRole('button')) {
      if ((button as HTMLButtonElement).disabled) {
        expect(button).toHaveAttribute('aria-describedby', stated.id);
      }
    }
  });

  it('keeps abilitiesFor as the single source of enablement', () => {
    // Three copies of the rule would eventually disagree, which shows up as a
    // button that does nothing.
    for (const lifecycle of ['provisioning', 'running', 'rotating', 'idle'] as const) {
      const can = abilitiesFor(session(lifecycle));
      const { unmount } = renderActions(lifecycle);

      expect(screen.getByRole('button', { name: /^end$/i }).hasAttribute('disabled')).toBe(
        !can.canStop,
      );
      expect(screen.getByRole('button', { name: /^delete$/i }).hasAttribute('disabled')).toBe(
        !can.canDelete,
      );
      if (can.canRevive) {
        expect(screen.getByRole('button', { name: /^revive$/i })).toBeEnabled();
      } else {
        expect(screen.getByRole('button', { name: /^reconnect$/i }).hasAttribute('disabled')).toBe(
          !can.canReconnect,
        );
      }
      unmount();
    }
  });

  it('confirms a delete inline, in place of the row', async () => {
    // §7: destructive actions confirm inline, never in a second dialog.
    const user = userEvent.setup();
    renderActions('running');
    await user.click(screen.getByRole('button', { name: /^delete$/i }));

    expect(screen.getByText(/delete this session and its history\?/i)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^end$/i })).toBeNull();
  });
});

describe('the details sheet', () => {
  it('collapses to a band that still says what the session is', () => {
    // §7: a collapsed panel is a handle carrying a summary — collapsing gives
    // up detail, not awareness.
    const { container } = render(
      <SessionDetails session={session('idle')} open={false} onToggle={vi.fn()} />,
    );
    const sheet = container.querySelector('.details');
    expect(sheet).toHaveClass('band--foot', 'details--closed');
    expect(sheet?.textContent).toContain('codex');
    expect(sheet?.textContent).toContain('idle');
    expect(sheet?.textContent).toContain('This session is stopped. Revive it to carry on.');
  });

  it('reverses its glyph, so the control describes its outcome', () => {
    const open = render(<SessionDetails session={session('running')} open onToggle={vi.fn()} />);
    expect(open.container.querySelector('.details__handle')?.textContent).toBe('⌄');
    open.unmount();

    const closed = render(
      <SessionDetails session={session('running')} open={false} onToggle={vi.fn()} />,
    );
    expect(closed.container.querySelector('.details__handle')?.textContent).toBe('⌃');
  });

  it('carries no lifecycle control', () => {
    // §12 moved them into the session header. A second copy here would be a
    // second place the rule could disagree with itself.
    render(<SessionDetails session={session('idle')} open onToggle={vi.fn()} />);
    expect(screen.queryByRole('button', { name: /^revive$/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /^delete$/i })).toBeNull();
  });

  it('lays its facts wide rather than tall, and scrolls inside itself', () => {
    // §12. The page does not scroll and neither does the terminal above it, so
    // a sheet that grew with its contents would take the height from the
    // surface it is describing.
    const css = readFileSync(resolve(process.cwd(), 'src/sessions/SessionDetails.css'), 'utf8');
    expect(css).toMatch(/\.details__list\s*\{[^}]*grid-template-columns: repeat\(auto-fill/);
    expect(css).toMatch(/\.details__body\s*\{[^}]*overflow-y: auto/);
    expect(css).toMatch(/\.details\s*\{[^}]*height: var\(--sheet-height\)/);
    expect(css).toMatch(/\.details--closed\s*\{[^}]*height: var\(--band-foot\)/);
  });
});
