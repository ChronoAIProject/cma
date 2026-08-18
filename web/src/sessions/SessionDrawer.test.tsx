import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { SessionDrawer } from './SessionDrawer';
import { SessionHeader } from './SessionHeader';
import { SessionActions } from './SessionActions';
import { abilitiesFor } from './availability';
import type { Session, SessionLifecycle } from './types';
import { NO_CAPABILITIES } from '../agui';

/**
 * The session header and the details drawer — docs/Web_Style.md §12 and §11.
 */

function session(lifecycle: SessionLifecycle, extra: Partial<Session> = {}): Session {
  return {
    id: 'ses_test',
    kernel: 'codex',
    lifecycle,
    createdAt: Date.parse('2026-08-18T08:53:00Z'),
    capabilities: NO_CAPABILITIES,
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
      onDetails={vi.fn()}
    />,
  );
}

function renderDrawer(lifecycle: SessionLifecycle, extra: Partial<Session> = {}) {
  return render(
    <SessionDrawer
      session={session(lifecycle, extra)}
      onClose={vi.fn()}
      onStop={vi.fn()}
      onDelete={vi.fn()}
    />,
  );
}

describe('the session header', () => {
  it('carries one primary, one secondary, and the overflow menu', () => {
    // §12 and §7: a header carrying four equally weighted keys makes the
    // reader choose before they can read.
    const { container } = render(
      <SessionHeader
        session={session('running')}
        actions={
          <SessionActions
            session={session('running')}
            onReconnect={vi.fn()}
            onRevive={vi.fn()}
            onStop={vi.fn()}
            onDetails={vi.fn()}
          />
        }
      />,
    );
    expect(container.querySelector('.sessionheader .actions')).not.toBeNull();
    expect(screen.getByRole('button', { name: /^reconnect$/i })).toBeEnabled();
    expect(screen.getByRole('button', { name: /^details$/i })).toBeEnabled();
    expect(screen.getByRole('button', { name: /more actions/i })).toHaveAttribute(
      'aria-haspopup',
      'menu',
    );
    // End and Delete are not header keys: End lives in the overflow, Delete
    // at the drawer's foot.
    expect(screen.queryByRole('button', { name: /^end$/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /^delete$/i })).toBeNull();
  });

  it('carries the status pill beside the subject', () => {
    const { container } = render(<SessionHeader session={session('running')} />);
    const pill = container.querySelector('.sessionheader .status');
    expect(pill).not.toBeNull();
    expect(pill?.textContent).toContain('running');
  });

  it('ends the session from the overflow menu', async () => {
    const user = userEvent.setup();
    const onStop = vi.fn();
    render(
      <SessionActions
        session={session('running')}
        onReconnect={vi.fn()}
        onRevive={vi.fn()}
        onStop={onStop}
        onDetails={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: /more actions/i }));
    await user.click(screen.getByRole('menuitem', { name: /end session/i }));
    expect(onStop).toHaveBeenCalled();
  });

  it.each([
    ['provisioning', 'This session is still starting.'],
    ['rotating', 'This session is being moved to another runtime. It will come back.'],
    ['idle', 'This session is stopped. Revive it to carry on.'],
  ] as const)('states why %s disables what it disables', (lifecycle, reason) => {
    // §7 and §11: the reason is drawn once per row, beside the controls it
    // explains. `abilitiesFor` disables for exactly one cause at a time, so
    // one sentence covers every control it disabled.
    renderActions(lifecycle);
    const stated = screen.getByText(reason);
    expect(stated).toBeInTheDocument();
    expect(stated).toHaveClass('control-reason');

    for (const button of screen.getAllByRole('button')) {
      if ((button as HTMLButtonElement).disabled) {
        expect(button).toHaveAttribute('aria-describedby', stated.id);
      }
    }
  });

  it('keeps abilitiesFor as the single source of enablement', async () => {
    // Three copies of the rule would eventually disagree, which shows up as a
    // button that does nothing.
    const user = userEvent.setup();
    for (const lifecycle of ['provisioning', 'running', 'rotating', 'idle'] as const) {
      const can = abilitiesFor(session(lifecycle));

      const actions = renderActions(lifecycle);
      if (can.canRevive) {
        expect(screen.getByRole('button', { name: /^revive$/i })).toBeEnabled();
      } else {
        expect(screen.getByRole('button', { name: /^reconnect$/i }).hasAttribute('disabled')).toBe(
          !can.canReconnect,
        );
      }
      await user.click(screen.getByRole('button', { name: /more actions/i }));
      expect(
        screen.getByRole('menuitem', { name: /end session/i }).getAttribute('aria-disabled'),
      ).toBe(String(!can.canStop));
      actions.unmount();

      const drawer = renderDrawer(lifecycle);
      expect(screen.getByRole('button', { name: /^end session$/i }).hasAttribute('disabled')).toBe(
        !can.canStop,
      );
      expect(screen.getByRole('button', { name: /^delete$/i }).hasAttribute('disabled')).toBe(
        !can.canDelete,
      );
      drawer.unmount();
    }
  });
});

describe('the details drawer', () => {
  it('confirms a delete inline, in place of the foot keys', async () => {
    // §7: destructive actions confirm inline, never in a second dialog.
    const user = userEvent.setup();
    renderDrawer('running');
    await user.click(screen.getByRole('button', { name: /^delete$/i }));

    expect(screen.getByText(/delete this session and its history\?/i)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^end session$/i })).toBeNull();
  });

  it('carries End and Delete at its foot, and no other lifecycle control', () => {
    // §12: the two acts that release or destroy sit at the drawer's foot,
    // away from everything reversible. Revive stays the header's primary.
    const { container } = renderDrawer('idle');
    const foot = container.querySelector('.drawer__foot');
    expect(foot?.textContent).toContain('End session');
    expect(foot?.textContent).toContain('Delete');
    expect(screen.queryByRole('button', { name: /^revive$/i })).toBeNull();
  });

  it('offers the identifier as a copyable control', () => {
    renderDrawer('running');
    const copy = screen.getByRole('button', { name: /ses_test/i });
    expect(copy.textContent).toContain('Copy');
  });

  it('takes width, not height, and overlays below 75rem', () => {
    // §12: the drawer takes width from a surface that has a thousand pixels
    // of it and none of the height. §16: below 75rem it overlays instead.
    const css = readFileSync(resolve(process.cwd(), 'src/sessions/SessionDrawer.css'), 'utf8');
    expect(css).toMatch(/\.drawer\s*\{[^}]*width: var\(--drawer-width\)/);
    expect(css).toMatch(/@media \(max-width: 74\.999rem\)[\s\S]*position: absolute/);
    expect(css).toMatch(/\.drawer__body\s*\{[^}]*overflow-y: auto/);
  });
});

describe('what a session reaches', () => {
  it('shows it without leaving the sessions view', () => {
    // §11 and §12: an owner watching an agent work on their behalf has one
    // question the interface did not answer — what does this thing have
    // access to right now?
    renderDrawer('running', {
      reach: [
        { id: 'api-github', name: 'GitHub', mandated: false },
        { id: 'nyxid', name: 'NyxID', mandated: true },
      ],
    });
    expect(screen.getByText('GitHub')).toBeInTheDocument();
    expect(screen.getByText('NyxID')).toBeInTheDocument();
  });

  it('draws a mandated service as fixed', () => {
    // Matching the picker's existing treatment, for the same stated reason: a
    // person deciding what a session may touch should see the whole answer.
    const { container } = renderDrawer('running', {
      reach: [
        { id: 'api-github', name: 'GitHub', mandated: false },
        { id: 'nyxid', name: 'NyxID', mandated: true },
      ],
    });
    const services = container.querySelectorAll('.drawer__service');
    expect(services[0]?.textContent).not.toContain('always');
    expect(services[1]?.textContent).toContain('always');
  });

  it('tells an empty grant apart from an unreported one', () => {
    const granted = renderDrawer('running', { reach: [] });
    expect(granted.container.textContent).toContain('reaches no services');
    granted.unmount();

    const unreported = renderDrawer('running');
    expect(unreported.container.textContent).toContain('Not reported by the control plane');
  });
});
