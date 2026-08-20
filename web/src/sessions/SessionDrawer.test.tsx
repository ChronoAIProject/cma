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

function renderActions(
  lifecycle: SessionLifecycle,
  extra: Partial<Session> = {},
  handlers: { onDelete?: () => void } = {},
) {
  return render(
    <SessionActions
      session={session(lifecycle, extra)}
      onReconnect={vi.fn()}
      onRevive={vi.fn()}
      onStop={vi.fn()}
      onDelete={handlers.onDelete ?? vi.fn()}
      onDetails={vi.fn()}
    />,
  );
}

function renderDrawer(lifecycle: SessionLifecycle, extra: Partial<Session> = {}) {
  return render(<SessionDrawer session={session(lifecycle, extra)} onClose={vi.fn()} />);
}

describe('the session header', () => {
  it('carries one lifecycle key, one details key, and the overflow menu', () => {
    /*
     * §12 and §7: a header carrying four equally weighted keys makes the
     * reader choose before they can read.
     *
     * The lifecycle is one key rather than whichever action happened to apply.
     * `Reconnect` and `Resume` were the same slot showing different words, so
     * which one somebody saw depended on a state nobody had told them about —
     * and they are different acts: one re-dials a live session's transport,
     * the other gives a paused one a sandbox back.
     */
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
            onDetails={vi.fn()}
          />
        }
      />,
    );
    expect(container.querySelector('.sessionheader .actions')).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Manage lifecycle' })).toBeEnabled();
    expect(screen.getByRole('button', { name: /^details$/i })).toBeEnabled();
    expect(screen.getByRole('button', { name: /more actions/i })).toHaveAttribute(
      'aria-haspopup',
      'menu',
    );
    // Nothing loose beside them: every lifecycle act is inside the one key.
    expect(screen.queryByRole('button', { name: /^reconnect$/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /^resume$/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /^pause session$/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /^delete$/i })).toBeNull();
  });

  it('offers every lifecycle act in one place, disabled rather than absent', async () => {
    // §7: the set keeps its shape as a session moves through its life, so
    // somebody who paused one yesterday finds Resume where Pause was rather
    // than hunting a menu that has rearranged itself.
    const user = userEvent.setup();
    renderActions('running');

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    for (const name of [/^resume$/i, /^reconnect$/i, /pause session/i, /^delete$/i]) {
      expect(screen.getByRole('menuitem', { name })).toBeInTheDocument();
    }
  });

  it('asks before deleting, and says what is lost', async () => {
    // §12.4: the tick is spent on the one action that loses something, and
    // moving it out of the drawer must not be how that is quietly dropped.
    const user = userEvent.setup();
    const onDelete = vi.fn();
    renderActions('running', {}, { onDelete });

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^delete$/i }));

    expect(screen.getByRole('dialog', { name: /delete this session/i })).toBeInTheDocument();
    expect(onDelete).not.toHaveBeenCalled();
  });

  it('carries the status pill beside the subject', () => {
    const { container } = render(<SessionHeader session={session('running')} />);
    const pill = container.querySelector('.sessionheader .pill');
    expect(pill).not.toBeNull();
    expect(pill?.textContent).toContain('running');
  });

  it('pauses the session from the lifecycle menu', async () => {
    const user = userEvent.setup();
    const onStop = vi.fn();
    render(
      <SessionActions
        session={session('running')}
        onReconnect={vi.fn()}
        onRevive={vi.fn()}
        onStop={onStop}
        onDelete={vi.fn()}
        onDetails={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /pause session/i }));
    expect(onStop).toHaveBeenCalled();
  });

  it.each([
    ['provisioning', 'This session is still starting.'],
    ['rotating', 'This session is being moved to another runtime. It will come back.'],
    ['idle', 'This session is paused. Resume it to carry on.'],
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
      // Every lifecycle act is one menu now, so every one is checked against
      // the same source rather than whichever happened to be a header key.
      await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
      /*
       * Prefix rather than exact: a disabled row carries its reason beside the
       * label, and the accessible name is their text run together.
       */
      const allowed: [RegExp, boolean][] = [
        [/^resume/i, can.canRevive],
        [/^reconnect/i, can.canReconnect],
        [/^pause session/i, can.canStop],
        [/^delete/i, can.canDelete],
      ];
      for (const [name, may] of allowed) {
        expect(
          screen.getByRole('menuitem', { name }).getAttribute('aria-disabled'),
          `${lifecycle} ${String(name)}`,
        ).toBe(String(!may));
      }
      actions.unmount();
    }
  });
});

describe('the header does not clip what it opens', () => {
  it('lets the overflow menu escape the band', () => {
    // The defect: the band carried `overflow: hidden`, so the menu — which
    // opens downward past the band's own 56px — was clipped away entirely.
    // Pressing `···` looked like it did nothing, or like the dropdown was
    // hiding behind the surface below.
    // Comments stripped first: this file explains in prose why the clip is
    // absent, and prose is not a declaration.
    const css = readFileSync(
      resolve(process.cwd(), 'src/sessions/SessionHeader.css'),
      'utf8',
    ).replace(/\/\*[\s\S]*?\*\//g, '');
    const band = /\.sessionheader\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(band).not.toMatch(/overflow:\s*hidden/);
    // And it paints above whatever the column is drawing, rather than
    // depending on document order against a terminal or a canvas.
    expect(band).toMatch(/position:\s*relative/);
    expect(band).toMatch(/z-index:/);
  });

  it('still keeps the one thing that can run long inside the band', () => {
    // What the clip was guarding against, handled where it belongs.
    const shared = readFileSync(resolve(process.cwd(), 'src/styles/components.css'), 'utf8');
    const reason = /\.control-reason\s*\{([^}]*)\}/.exec(shared)?.[1] ?? '';
    expect(reason).toMatch(/text-overflow:\s*ellipsis/);
    expect(reason).toMatch(/white-space:\s*nowrap/);
  });
});

describe('the details drawer', () => {
  it('gates a delete behind an acknowledgement rather than a second press', async () => {
    /*
     * §7. The safeguard is the tick, not a colour: a red key is one a person
     * can click as fast as any other, and what is being protected here is a
     * session's whole history.
     *
     * Driven from the header, where delete lives now — the lifecycle is one
     * menu, and moving it must not be how the acknowledgement is dropped.
     */
    const user = userEvent.setup();
    const onDelete = vi.fn();
    renderActions('running', {}, { onDelete });
    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^delete/i }));

    const confirm = screen.getByRole('button', { name: /delete permanently/i });
    expect(confirm).toBeDisabled();

    await user.click(screen.getByRole('checkbox'));
    expect(confirm).toBeEnabled();

    await user.click(confirm);
    expect(onDelete).toHaveBeenCalledOnce();
  });

  it('forgets the acknowledgement when the question is dismissed', async () => {
    // Otherwise the dialog reopens already ticked, which is the one state
    // this control exists to prevent.
    const user = userEvent.setup();
    renderActions('running');

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^delete/i }));
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: /^cancel$/i }));

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^delete/i }));
    expect(screen.getByRole('checkbox')).not.toBeChecked();
    expect(screen.getByRole('button', { name: /delete permanently/i })).toBeDisabled();
  });

  it('acts on nothing, and says nothing about itself', () => {
    /*
     * Reference material. Every lifecycle act is one key in the session header
     * now, and splitting them across two surfaces is how somebody comes
     * looking for an action where it used to be — a panel that also acts is
     * one people are afraid to open.
     *
     * The heading went with them: the `aside` is named for assistive
     * technology, and "Details" over a list of details told a sighted reader
     * what they were already looking at while spending the top of the column
     * on it.
     */
    const { container } = renderDrawer('idle');

    expect(container.querySelector('.drawer__foot')).toBeNull();
    expect(container.querySelector('.drawer__title')).toBeNull();
    expect(screen.queryByRole('heading', { name: /^details$/i })).toBeNull();
    for (const name of [/^pause session/i, /^delete/i, /^resume/i, /^reconnect/i]) {
      expect(screen.queryByRole('button', { name })).toBeNull();
    }
    // The one control it keeps is the way out of it.
    expect(screen.getByRole('button', { name: /close session details/i })).toBeInTheDocument();
  });

  it('offers the identifier as a copyable control', () => {
    renderDrawer('running');
    const copy = screen.getByRole('button', { name: /ses_test/i });
    expect(copy.textContent).toContain('Copy');
  });

  it('fills its column and scrolls inside itself', () => {
    /*
     * The width is the frame's decision now (§9), so declaring one here would
     * be a width the solver cannot honour. And the body scrolls rather than
     * the column: a column that grew with its contents would push the foot's
     * destructive keys off the window.
     *
     * The old overlay below 75rem goes with it. The concession chain closes
     * this column when there is no room for it, and a second mechanism for
     * the same thing is one that can disagree with the first.
     */
    const css = readFileSync(resolve(process.cwd(), 'src/sessions/SessionDrawer.css'), 'utf8');
    expect(css).not.toMatch(/\.drawer\s*\{[^}]*width:/);
    expect(css).not.toMatch(/@media \(max-width: 74\.999rem\)/);
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
