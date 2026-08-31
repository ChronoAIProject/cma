import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SessionDrawer } from './SessionDrawer';
import { SessionHeader } from './SessionHeader';
import { SessionActions } from './SessionActions';
import { abilitiesFor } from './availability';
import type { Session, SessionLifecycle } from './types';
import { NO_CAPABILITIES } from '@chronoai/cma-sdk';
import type {
  DiffReview,
  Launch,
  LaunchCapabilityRevocationState,
  PullRequestDraft,
  PullRequestRecord,
  SessionSpend,
} from '@chronoai/cma-sdk';
import { listLaunchPage, readLaunch } from '../api/client';

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>();
  return { ...actual, listLaunchPage: vi.fn(), readLaunch: vi.fn() };
});

const launchFixture = JSON.parse(
  readFileSync(resolve(process.cwd(), '../sdk/typescript/tests/fixtures/launch.json'), 'utf8'),
) as Launch;
const diffFixture = JSON.parse(
  readFileSync(resolve(process.cwd(), '../sdk/typescript/tests/fixtures/diff_review.json'), 'utf8'),
) as DiffReview;
const pullRequestDraft = JSON.parse(
  readFileSync(
    resolve(process.cwd(), '../sdk/typescript/tests/fixtures/pull_request_draft.json'),
    'utf8',
  ),
) as PullRequestDraft;

function openPullRequest(): PullRequestRecord {
  return {
    ...pullRequestDraft,
    provider_id: 101,
    provider_node_id: 'PR_provider_node',
    number: 17,
    canonical_url: 'https://github.com/upstream-owner/project/pull/17',
    state: 'open',
    base_revision: pullRequestDraft.expected_base_revision,
    head_revision: pullRequestDraft.expected_head_revision,
    observed_title: pullRequestDraft.title,
    observed_body_digest: pullRequestDraft.body_digest,
    mergeability: 'mergeable',
    checks: 'passing',
    confirmation_snapshot_digest: pullRequestDraft.intent_digest,
    confirmed_at: '2026-08-27T00:00:15Z',
    created_at: '2026-08-27T00:00:15Z',
    updated_at: '2026-08-27T00:00:15Z',
    provider_created_at: '2026-08-27T00:00:15Z',
    provider_updated_at: '2026-08-27T00:00:15Z',
    status_failure: null,
    provider_request_id: 'github-request-17',
    head_matches_launch: true,
  };
}

function owningLaunch(reach: LaunchCapabilityRevocationState | null = 'active'): Launch {
  const upstream = launchFixture.revision_content.upstream.repository;
  return {
    ...launchFixture,
    session: { session_id: 'ses_test' },
    fork: {
      repository: {
        ...upstream,
        repository_id: 11,
        repository_node_id: 'R_fork',
        owner_id: 21,
        owner_node_id: 'U_contributor',
        canonical_owner: 'contributor',
        fork_parent_id: upstream.repository_id,
        fork_source_id: upstream.repository_id,
      },
      reused: false,
      source_revision: launchFixture.source_sha,
      canonical_url: 'https://github.com/contributor/project',
      ready_at: '2026-08-26T12:00:00Z',
    },
    branch: {
      name: `cma/${launchFixture.id}`,
      expected_head: launchFixture.source_sha,
      current_head: launchFixture.source_sha,
      canonical_url: `https://github.com/contributor/project/tree/cma/${launchFixture.id}`,
      created_at: '2026-08-26T12:00:00Z',
    },
    workspace: { latest_checkpoint: { id: 'checkpoint-1' } },
    reach:
      reach === null
        ? null
        : {
            service_id: 'service-1',
            state: reach,
            policy_generation: 1,
            policy_fence: 1,
            runtime_audience: 'runtime-fixture',
            expires_at_epoch_seconds: 1_800_000_000,
            numeric_binding_observed_at: '2026-08-26T12:00:00Z',
            refreshed_at: '2026-08-26T12:00:00Z',
            tombstone_acknowledged: false,
            agent_key_revocation_confirmed: false,
          },
    delivery: {
      launch_id: launchFixture.id,
      session_id: 'ses_test',
      revision_digest: launchFixture.revision_digest,
      origin_key: `trigger:${launchFixture.id}:initial-prompt`,
      payload_hash: 'b'.repeat(64),
      state: 'write_confirmed',
      attempt: 1,
      recovery_generation: 0,
      dispatch_fence_sequence: 1,
      queued_at: '2026-08-26T12:00:00Z',
      updated_at: '2026-08-26T12:00:00Z',
    },
    diff: diffFixture.artifact,
  };
}

beforeEach(() => {
  vi.mocked(listLaunchPage)
    .mockReset()
    .mockImplementation(() => new Promise(() => {}));
  vi.mocked(readLaunch)
    .mockReset()
    .mockImplementation(() => new Promise(() => {}));
});

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
  handlers: { onTerminate?: () => void; onDelete?: () => void } = {},
) {
  return render(
    <SessionActions
      session={session(lifecycle, extra)}
      onReconnect={vi.fn()}
      onRevive={vi.fn()}
      onStop={vi.fn()}
      onTerminate={handlers.onTerminate ?? vi.fn()}
      onDelete={handlers.onDelete ?? vi.fn()}
      onDetails={vi.fn()}
    />,
  );
}

function renderDrawer(
  lifecycle: SessionLifecycle,
  extra: Partial<Session> = {},
  readSpend?: (id: string) => Promise<SessionSpend>,
  navigate?: Parameters<typeof SessionDrawer>[0]['navigate'],
) {
  return render(
    <SessionDrawer
      session={session(lifecycle, extra)}
      onClose={vi.fn()}
      readSpend={readSpend}
      navigate={navigate}
    />,
  );
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
            onTerminate={vi.fn()}
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
    for (const name of ['Manage lifecycle', 'Details', 'More actions']) {
      expect(screen.getByRole('button', { name })).toHaveClass('button--sm');
    }
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

    // The header places the shared badge; it does not replace its mono voice
    // with a local text size that makes this one pill a different component.
    const css = readFileSync(resolve(process.cwd(), 'src/sessions/SessionHeader.css'), 'utf8');
    expect(css).not.toMatch(/\.sessionheader \.pill\s*\{/);
  });

  it('shows an unfamiliar lifecycle under the word the server reported', () => {
    const current = session('unknown', { reportedStatus: 'hibernating' });
    const header = render(<SessionHeader session={current} />);
    expect(header.getByText('hibernating')).toBeInTheDocument();
    header.unmount();

    renderDrawer('unknown', { reportedStatus: 'hibernating' });
    expect(screen.getByText('hibernating')).toBeInTheDocument();
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
        onTerminate={vi.fn()}
        onDelete={vi.fn()}
        onDetails={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /pause session/i }));
    expect(onStop).toHaveBeenCalled();
  });

  it('confirms retained termination and explains where the history remains', async () => {
    const user = userEvent.setup();
    const onTerminate = vi.fn();
    renderActions('running', {}, { onTerminate });

    await user.click(screen.getByRole('button', { name: 'Manage lifecycle' }));
    await user.click(screen.getByRole('menuitem', { name: /^terminate session/i }));

    const dialog = screen.getByRole('dialog', { name: 'Terminate this session?' });
    expect(dialog).toHaveTextContent(/cannot be resumed/i);
    expect(dialog).toHaveTextContent(/remain under Terminated until deleted/i);
    expect(within(dialog).queryByRole('checkbox')).toBeNull();
    await user.click(within(dialog).getByRole('button', { name: /^terminate session$/i }));
    expect(onTerminate).toHaveBeenCalledOnce();
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
        [/^terminate session/i, can.canTerminate],
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
    const services = container.querySelectorAll('.facts__entry');
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

describe('the Trigger repository relationship', () => {
  it('discovers the owner-scoped Launch and exposes a reviewed contribution', async () => {
    vi.mocked(listLaunchPage).mockResolvedValueOnce({
      items: [owningLaunch()],
      next_cursor: null,
    });

    renderDrawer('running');

    expect(await screen.findByText('upstream-owner/project')).toBeInTheDocument();
    expect(listLaunchPage).toHaveBeenCalledWith(
      { session_id: 'ses_test', limit: 1 },
      expect.any(AbortSignal),
    );
    expect(screen.getByText('contributor/project')).toBeInTheDocument();
    expect(screen.getByText('checkpoint-1')).toBeInTheDocument();
    expect(screen.getByText('write confirmed')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Launch progress' })).toHaveAttribute(
      'href',
      `/launches/${launchFixture.id}`,
    );
    expect(screen.getByRole('link', { name: 'Contribute' })).toHaveAttribute(
      'href',
      `/launches/${launchFixture.id}/contribute`,
    );
  });

  it('keeps polling the exact session relationship until a Launch is associated', async () => {
    vi.useFakeTimers();
    vi.mocked(listLaunchPage)
      .mockResolvedValueOnce({ items: [], next_cursor: null })
      .mockResolvedValueOnce({ items: [owningLaunch()], next_cursor: null });
    const view = renderDrawer('running');

    try {
      await act(async () => {
        await Promise.resolve();
      });
      expect(listLaunchPage).toHaveBeenCalledTimes(1);
      expect(screen.queryByText('upstream-owner/project')).toBeNull();

      await act(async () => {
        vi.advanceTimersByTime(15_000);
        await Promise.resolve();
      });

      expect(listLaunchPage).toHaveBeenCalledTimes(2);
      expect(listLaunchPage).toHaveBeenLastCalledWith(
        { session_id: 'ses_test', limit: 1 },
        expect.any(AbortSignal),
      );
      expect(screen.getByText('upstream-owner/project')).toBeInTheDocument();
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('retries Launch discovery after a failed session lookup', async () => {
    vi.useFakeTimers();
    vi.mocked(listLaunchPage)
      .mockRejectedValueOnce(new Error('inventory temporarily unavailable'))
      .mockResolvedValueOnce({ items: [owningLaunch()], next_cursor: null });
    const view = renderDrawer('running');

    try {
      await act(async () => {
        await Promise.resolve();
      });
      expect(screen.getByText('Could not be read')).toBeInTheDocument();

      await act(async () => {
        vi.advanceTimersByTime(15_000);
        await Promise.resolve();
      });

      expect(listLaunchPage).toHaveBeenCalledTimes(2);
      expect(screen.getByText('upstream-owner/project')).toBeInTheDocument();
      expect(screen.queryByText('Could not be read')).toBeNull();
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('shows the linked pull request target and complete observed provider status', async () => {
    vi.mocked(listLaunchPage).mockResolvedValueOnce({
      items: [{ ...owningLaunch(), pull_request: openPullRequest() }],
      next_cursor: null,
    });

    renderDrawer('running');

    const heading = await screen.findByRole('heading', { name: 'Trigger repository' });
    const section = heading.closest('section');
    expect(section).not.toBeNull();
    const repository = within(section as HTMLElement);
    expect(repository.getByRole('link', { name: `#17 ${pullRequestDraft.title}` })).toHaveAttribute(
      'href',
      'https://github.com/upstream-owner/project/pull/17',
    );
    expect(repository.getByText('PR state').closest('div')).toHaveTextContent('open');
    expect(repository.getByText('PR base').closest('div')).toHaveTextContent(
      pullRequestDraft.base_ref,
    );
    expect(repository.getByText('PR head').closest('div')).toHaveTextContent(
      pullRequestDraft.head_ref,
    );
    expect(repository.getByText('Mergeability').closest('div')).toHaveTextContent('Mergeable');
    expect(repository.getByText('Checks').closest('div')).toHaveTextContent('passing');
    expect(repository.getByText('CMA head binding').closest('div')).toHaveTextContent(
      'Matches Launch',
    );
    expect(repository.getByText('Status sync').closest('div')).toHaveTextContent('Current');
    expect(repository.getByText('Provider request').closest('div')).toHaveTextContent(
      'github-request-17',
    );
  });

  it('keeps the drawer snapshot current while it remains open', async () => {
    const initial = { ...owningLaunch(), pull_request: openPullRequest() };
    const refreshed = {
      ...initial,
      revision_number: initial.revision_number + 1,
      pull_request: {
        ...openPullRequest(),
        checks: 'failing' as const,
        mergeability: 'conflicting' as const,
        provider_request_id: 'github-request-poll',
        updated_at: '2026-08-27T00:01:00Z',
      },
    };
    vi.mocked(listLaunchPage).mockResolvedValueOnce({ items: [initial], next_cursor: null });
    vi.mocked(readLaunch).mockResolvedValueOnce(refreshed);
    renderDrawer('running');

    expect(await screen.findByText('github-request-poll')).toBeInTheDocument();
    expect(readLaunch).toHaveBeenCalledWith(initial.id, expect.any(AbortSignal));
    expect(screen.getByText('failing')).toBeInTheDocument();
    expect(screen.getByText('Conflicting')).toBeInTheDocument();
  });

  it('keeps stale repository facts readable but withholds Contribute after refresh failure', async () => {
    const initial = owningLaunch();
    vi.mocked(listLaunchPage).mockResolvedValueOnce({ items: [initial], next_cursor: null });
    vi.mocked(readLaunch).mockRejectedValueOnce(new Error('Launch refresh unavailable'));

    renderDrawer('running');

    expect(
      await screen.findByText('The latest Launch refresh failed; provider status may be stale.'),
    ).toBeInTheDocument();
    expect(screen.getByText('upstream-owner/project')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Launch progress' })).toHaveAttribute(
      'href',
      `/launches/${initial.id}`,
    );
    expect(screen.queryByRole('link', { name: 'Contribute' })).toBeNull();
  });

  it('shows manual head drift and safe paused-status remediation in the drawer', async () => {
    vi.mocked(listLaunchPage).mockResolvedValueOnce({
      items: [
        {
          ...owningLaunch(),
          pull_request: {
            ...openPullRequest(),
            head_matches_launch: false,
            status_failure: {
              class: 'service_inactive',
              code: 'github_service_inactive',
              detail: 'The exact NyxID GitHub service is inactive.',
              remediation: 'Enable that service in NyxID or select another active connection.',
              correlation_id: 'correlation-inactive',
              observed_at: '2026-08-27T00:00:00Z',
            },
          },
        },
      ],
      next_cursor: null,
    });

    renderDrawer('running');

    expect(await screen.findByText('Changed outside CMA')).toBeInTheDocument();
    const status = screen.getByText('Status sync').closest('div');
    expect(status).not.toBeNull();
    expect(status).toHaveTextContent('Paused by CMA');
    expect(status).toHaveTextContent('exact NyxID GitHub service is inactive');
    expect(status).toHaveTextContent('Enable that service in NyxID');
    expect(screen.queryByRole('link', { name: 'Contribute' })).toBeNull();
    expect(screen.getByRole('link', { name: 'Launch progress' })).toBeInTheDocument();
  });

  it.each(['merged', 'closed_unmerged'] as const)(
    'preserves %s pull-request history without offering a same-Launch contribution',
    async (state) => {
      vi.mocked(listLaunchPage).mockResolvedValueOnce({
        items: [{ ...owningLaunch(), pull_request: { ...openPullRequest(), state } }],
        next_cursor: null,
      });

      renderDrawer('running');

      expect(
        await screen.findByRole('link', { name: `#17 ${pullRequestDraft.title}` }),
      ).toHaveAttribute('href', 'https://github.com/upstream-owner/project/pull/17');
      expect(screen.getByText('PR state').closest('div')).toHaveTextContent(
        state.replaceAll('_', ' '),
      );
      expect(screen.queryByRole('link', { name: 'Contribute' })).toBeNull();
      expect(screen.getByRole('link', { name: 'Launch progress' })).toBeInTheDocument();
    },
  );

  it('discloses ambiguous initial-task risk and links to the existing Launch recovery action', async () => {
    const navigate = vi.fn();
    const launch = owningLaunch();
    if (launch.delivery === null) throw new Error('owning Launch fixture omitted delivery');
    vi.mocked(listLaunchPage).mockResolvedValueOnce({
      items: [
        {
          ...launch,
          state: 'failed_retryable',
          blocked_reason: 'recovery_required',
          delivery: { ...launch.delivery, state: 'recovery_required' },
        },
      ],
      next_cursor: null,
    });

    renderDrawer('running', {}, undefined, navigate);

    expect(
      await screen.findByText(/cannot determine whether the previous initial task started/i),
    ).toHaveTextContent(/another dispatch may execute the task twice/i);
    const recovery = screen.getByRole('link', { name: 'Review initial-task recovery' });
    expect(recovery).toHaveAttribute('href', `/launches/${launch.id}`);
    expect(screen.queryByRole('button', { name: 'Recover initial task' })).toBeNull();

    await userEvent.click(recovery);
    expect(navigate).toHaveBeenCalledWith({ kind: 'launch', launchId: launch.id });
  });

  it.each([
    ['missing', null],
    ['revocation pending', 'revocation_pending'],
    ['revoked', 'revoked'],
    ['expired', 'expired'],
  ] as const)('withholds Contribute when GitHub reach is %s', async (_label, reach) => {
    vi.mocked(listLaunchPage).mockResolvedValueOnce({
      items: [owningLaunch(reach)],
      next_cursor: null,
    });

    renderDrawer('running');

    expect(
      await screen.findByText(reach === null ? 'Pending' : reach.replaceAll('_', ' ')),
    ).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Contribute' })).toBeNull();
    expect(screen.getByRole('link', { name: 'Launch progress' })).toBeInTheDocument();
  });
});

describe('how a session is configured', () => {
  it('says what is answering, in words nobody has to look up', () => {
    /*
     * The wire spells these as settings; the panel spells them as answers. A
     * person reading "guarded" learns the name of something they did not
     * choose, and "x_high" is not a word at all — it is how serde renders an
     * identifier beginning with a capital.
     */
    const { container } = renderDrawer('running', {
      agent: {
        model: 'gpt-5-codex',
        reasoningEffort: 'x_high',
        permission: 'guarded',
        credential: 'delegated',
        skills: [],
      },
    });

    expect(container.textContent).toContain('gpt-5-codex');
    expect(container.textContent).toContain('Maximum');
    expect(container.textContent).not.toContain('x_high');
    expect(container.textContent).toContain('Guarded');
    expect(container.textContent).toContain('Asks before anything outside its workspace');
  });

  it('marks an unrestricted agent as the exceptional choice it is', () => {
    /*
     * The core exposes which mode is exceptional precisely so two surfaces
     * cannot disagree about it. This is the one row below the state band that
     * is allowed a colour, and it earns it: an agent that confines nothing is
     * a fact somebody should not have to read a sentence to notice.
     */
    const { container } = renderDrawer('running', {
      agent: { permission: 'unrestricted' },
    });

    const marked = container.querySelector('.facts__answer[data-tone="fail"]');
    expect(marked?.textContent).toBe('Unrestricted');
    expect(container.textContent).toContain('Asks for nothing, and confines nothing');
  });

  it('leaves a guarded agent uncoloured, so the marking still means something', () => {
    // The mutation that would make the test above pass for the wrong reason:
    // colouring every permission row. Then the colour says nothing.
    const { container } = renderDrawer('running', {
      agent: { permission: 'guarded' },
    });
    expect(container.querySelector('.facts__answer[data-tone]')).toBeNull();
  });

  it('names whose account pays for the model, and never the credential', () => {
    const managed = renderDrawer('running', { agent: { credential: 'delegated' } });
    expect(managed.container.textContent).toContain('ChronoAI managed');

    const own = renderDrawer('running', { agent: { credential: 'own' } });
    expect(own.container.textContent).toContain('Your own account');
    expect(own.container.textContent).toContain('with the model vendor');
  });

  it('shows an accruing capacity figure beside who pays for the model', async () => {
    const readSpend = vi.fn().mockResolvedValue({
      priced: true,
      rateCard: 'rate_current',
      currency: 'USD',
      microsPerHour: '45000',
      accruedMicros: '45000',
      display: '$0.045',
      billableSeconds: 3600,
      accruing: true,
      unboundedIntervals: 0,
    } satisfies SessionSpend);
    renderDrawer('running', { agent: { credential: 'delegated' } }, readSpend);

    expect(await screen.findByText('$0.045')).toBeInTheDocument();
    expect(screen.getByText(/Accruing while this session is running/)).toBeInTheDocument();
    expect(readSpend).toHaveBeenCalledWith('ses_test');
  });

  it('marks a stopped figure final and says when its coverage is incomplete', async () => {
    const readSpend = vi.fn().mockResolvedValue({
      priced: true,
      rateCard: 'rate_current',
      currency: 'USD',
      microsPerHour: '45000',
      accruedMicros: '90000',
      display: '$0.09',
      billableSeconds: 7200,
      accruing: false,
      unboundedIntervals: 1,
    } satisfies SessionSpend);
    renderDrawer('idle', {}, readSpend);

    expect(await screen.findByText('$0.09')).toBeInTheDocument();
    expect(screen.getByText(/Final reserved-capacity charge/)).toBeInTheDocument();
    expect(screen.getByText(/covers less than the session’s whole life/)).toBeInTheDocument();
  });

  it('renders an absent price as not priced and never as zero', async () => {
    const readSpend = vi.fn().mockResolvedValue({
      priced: false,
      accruing: false,
      unboundedIntervals: 0,
    } satisfies SessionSpend);
    const { container } = renderDrawer('running', {}, readSpend);

    expect(await screen.findByText('Not priced')).toBeInTheDocument();
    expect(container.textContent).not.toContain('$0.00');
  });

  it('says a control plane that reported nothing said nothing', () => {
    /*
     * An older control plane sends no agent at all. Every row then reads "not
     * reported" rather than drawing blank — a panel of empty values is read as
     * a rendering fault, and a model row that is blank because the kernel
     * chooses its own default is a different fact again.
     */
    const { container } = renderDrawer('running');
    expect(container.textContent).toContain('Not reported');
    expect(container.textContent).toContain('The kernel’s own default');
  });
});

describe('what a session was given', () => {
  it('lists the skills installed into its sandbox, with the count', () => {
    const { container } = renderDrawer('running', {
      agent: {
        skills: [
          { id: 'skl_1', name: 'NyxID CLI', description: 'Reach services through NyxID' },
          { id: 'skl_2', name: 'Ornn CLI' },
        ],
      },
    });

    expect(screen.getByText('NyxID CLI')).toBeInTheDocument();
    expect(screen.getByText('Reach services through NyxID')).toBeInTheDocument();
    expect(screen.getByText('Ornn CLI')).toBeInTheDocument();
    // The count beside the title, so the size of a list is legible without
    // counting its rows.
    expect(container.querySelector('.facts__count')?.textContent).toBe('2');
  });

  it('links each skill to its page in the registry, in a new tab', () => {
    renderDrawer('running', {
      agent: { skills: [{ id: 'nyxid', name: 'NyxID CLI', version: '0.5' }] },
    });

    const link = screen.getByRole('link', { name: /NyxID CLI/ });
    expect(link).toHaveAttribute('href', 'https://ornn.chrono-ai.fun/skills/nyxid');
    // A session is a live thing somebody is working in; navigating it away to
    // read a skill's page would cost them the surface they were using.
    expect(link).toHaveAttribute('target', '_blank');
    // `noopener` denies the opened page a handle back to this one.
    expect(link.getAttribute('rel')).toContain('noopener');
    expect(link.textContent).toContain('@0.5');
  });

  it('shows no version rather than an empty one when the registry reported none', () => {
    // The mutation this kills: rendering `@` with nothing after it, or
    // inventing "latest".
    const { container } = renderDrawer('running', {
      agent: { skills: [{ id: 'nyxid', name: 'NyxID CLI' }] },
    });
    expect(container.textContent).not.toContain('@');
    // And it is still a link — the version is not what makes it one.
    expect(screen.getByRole('link', { name: /NyxID CLI/ })).toHaveAttribute(
      'href',
      'https://ornn.chrono-ai.fun/skills/nyxid',
    );
  });

  it('falls back to the identifier when the catalogue could not name a skill', () => {
    // A catalogue that could not be asked costs the name and not the answer.
    renderDrawer('running', { agent: { skills: [{ id: 'skl_unknown' }] } });
    expect(screen.getByText('skl_unknown')).toBeInTheDocument();
  });

  it('says a starting session is still installing, not that nothing was recorded', () => {
    /*
     * Provisioning is the only writer of the grant, so before it finishes
     * there genuinely is nothing recorded. Saying that flatly reads as a
     * permanent answer to somebody watching a session start — and it is the
     * sentence that got reported as a bug against a session whose agent was
     * visibly using the skills.
     */
    const starting = renderDrawer('provisioning', { agent: {} });
    expect(starting.container.textContent).toContain('still starting');
    expect(starting.container.textContent).not.toContain('Not recorded');

    // And once it has settled, an absent grant IS the permanent answer.
    const settled = renderDrawer('running', { agent: {} });
    expect(settled.container.textContent).toContain('Not recorded for this session');
    expect(settled.container.textContent).not.toContain('still starting');
  });

  it('tells a session given no skills apart from one that recorded none', () => {
    /*
     * The same distinction `reach` draws, and for the same reason: a session
     * created before the grant was recorded is not a session that was given
     * nothing, and one sentence cannot be both.
     */
    const none = renderDrawer('running', { agent: { skills: [] } });
    expect(none.container.textContent).toContain('was given no skills');

    const unrecorded = renderDrawer('running', { agent: {} });
    expect(unrecorded.container.textContent).toContain('Not recorded for this session');
  });
});

describe('the drawer reads as sections rather than a list', () => {
  it('says what the Reaches band is, since a list of slugs does not', () => {
    /*
     * The band lists identifiers like `api-github-pat`, and nothing on the
     * surface said those were services on the identity platform rather than
     * something this product invented. The count and the entries answer "how
     * many" and "which"; neither answers "of what".
     */
    const { container } = renderDrawer('running', {
      reach: [{ id: 'api-github-pat', name: 'GitHub', mandated: true }],
    });
    expect(container.textContent).toContain('NyxID services which this session is scoped to.');
  });

  it('leaves a band without a note unannotated, rather than captioning them all', () => {
    // The mutation that would make the test above pass for the wrong reason.
    // A caption on every band is a caption nobody reads.
    const { container } = renderDrawer('running', { agent: { model: 'gpt-5-codex' } });
    expect(container.querySelectorAll('.facts__subtitle')).toHaveLength(1);
  });

  it('groups its facts under titles', () => {
    const { container } = renderDrawer('running', {
      agent: { model: 'gpt-5-codex' },
      runtimeVersion: '680fbde',
    });

    const titles = [...container.querySelectorAll('.facts__title')].map((t) =>
      t.textContent?.replace(/\d+$/, ''),
    );
    expect(titles).toEqual(['Agent', 'Skills', 'Reaches', 'Runtime', 'Identifier']);
  });

  it('drops the runtime band when there is nothing to put in it', () => {
    // Two empty rows under a title is worse than no title: it reads as a
    // section that failed to load.
    const { container } = renderDrawer('running');
    const titles = [...container.querySelectorAll('.facts__title')].map((t) => t.textContent);
    expect(titles).not.toContain('Runtime');
  });

  it('spends its one colour on the state and nowhere else', () => {
    /*
     * The restraint is the design decision, so it is the thing worth pinning.
     * §12.1's success, warning and error aliases are the state vocabulary, and
     * a section tinted for decoration would leave the interface no way to say
     * that the unrestricted row above is different from the four around it.
     */
    const css = readFileSync(resolve(process.cwd(), 'src/sessions/SessionDrawer.css'), 'utf8');
    const tinted = [...css.matchAll(/^([.:[][^{]*)\{([^}]*)\}/gm)].filter(([, , body]) =>
      /var\(--dsw-alias-(?:state-(?:success|warn|error)-primary|state-business-primary)\)/.test(
        body ?? '',
      ),
    );
    for (const [, selector] of tinted) {
      expect(selector ?? '').toMatch(/drawer__band|drawer__ending|facts__answer\[data-tone/);
    }
    // And the state band is genuinely painted, so the assertion above is not
    // passing over an empty set.
    expect(tinted.length).toBeGreaterThan(2);
  });
});
