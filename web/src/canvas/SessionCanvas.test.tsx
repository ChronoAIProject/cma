import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { SessionCanvas } from './SessionCanvas';
import { MAX_LIVE_NODES } from './layout';
import type { LiveSession } from '../sessions/live';
import type { Session, SessionLifecycle } from '../sessions/types';
import type { TerminalDriver } from '../terminal/types';
import { unattachedNote } from './layout';

/**
 * Recording drivers rather than a real one: what these tests assert is where
 * bytes went, and a driver with a transport would launder that.
 */
function makeFleet(count: number, lifecycle: SessionLifecycle = 'running') {
  const nodes: LiveSession[] = [];
  const sent = new Map<string, string[]>();
  const attached = new Set<string>();

  for (let i = 0; i < count; i += 1) {
    const id = `s-${i}`;
    sent.set(id, []);
    const driver: TerminalDriver = {
      attach: () => attached.add(id),
      send: (data: string) => sent.get(id)?.push(data),
      resize: () => {},
      detach: () => attached.delete(id),
    };
    nodes.push({
      session: session(id, lifecycle, i),
      driver,
    });
  }

  return { nodes, sent, attached };
}

/** Distinct creation times, so nodes are told apart the way the rail tells them apart. */
function session(id: string, lifecycle: SessionLifecycle, index: number): Session {
  return {
    id,
    kernel: 'codex',
    lifecycle,
    createdAt: Date.UTC(2026, 7, 14, 3, index),
  };
}

/** The canvas is controlled; the harness owns focus the way Console does. */
function Harness({
  nodes,
  initialFocus,
}: {
  nodes: readonly LiveSession[];
  initialFocus: string | null;
}) {
  const [focusedId, setFocusedId] = useState<string | null>(initialFocus);
  const [history, setHistory] = useState<readonly string[]>(
    initialFocus === null ? [] : [initialFocus],
  );
  return (
    <SessionCanvas
      nodes={nodes}
      focusedId={focusedId}
      focusHistory={history}
      onFocus={(id) => {
        setFocusedId(id);
        setHistory((prev) => (prev[0] === id ? prev : [id, ...prev.filter((seen) => seen !== id)]));
      }}
    />
  );
}

/**
 * xterm reads `keyCode`, so an init carrying only `key` produces an event that
 * looks plausible and does nothing — which would make an assertion that no
 * input arrived pass for the wrong reason.
 */
const ENTER = { key: 'Enter', code: 'Enter', keyCode: 13, which: 13 };

function nodeFor(id: string): HTMLElement {
  const node = document.querySelector(`[data-node-id="${id}"]`);
  if (node === null) {
    throw new Error(`no node for session ${id}`);
  }
  return node as HTMLElement;
}

function handleFor(id: string): HTMLElement {
  return within(nodeFor(id)).getByRole('button');
}

function terminalInputFor(id: string): HTMLTextAreaElement {
  const input = nodeFor(id).querySelector('textarea');
  if (input === null) {
    throw new Error(`session ${id} has no terminal input`);
  }
  return input;
}

describe('SessionCanvas', () => {
  it('renders every session as a node with an accessible name', () => {
    const { nodes } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    expect(screen.getAllByRole('region')).toHaveLength(3);
    expect(nodeFor('s-1')).toHaveAccessibleName(/^Session .+ on codex, running$/);
  });

  it('attaches every visible node, not only the focused one', () => {
    const { nodes, attached } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    // Unfocused nodes must keep rendering live output.
    expect(attached).toEqual(new Set(['s-0', 's-1', 's-2']));
  });

  it('marks exactly one node as focused', async () => {
    const user = userEvent.setup();
    const { nodes } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    await user.click(handleFor('s-2'));

    const pressed = screen
      .getAllByRole('button')
      .filter((button) => button.getAttribute('aria-pressed') === 'true');
    expect(pressed).toHaveLength(1);
    expect(handleFor('s-2')).toHaveAttribute('aria-pressed', 'true');
  });

  it('sends a keystroke to the focused session and to no other', async () => {
    const user = userEvent.setup();
    const { nodes, sent } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    await user.click(handleFor('s-1'));
    await user.type(terminalInputFor('s-1'), 'y');

    // The property this whole view lives or dies by.
    expect(sent.get('s-1')).toEqual(['y']);
    expect(sent.get('s-0')).toEqual([]);
    expect(sent.get('s-2')).toEqual([]);
  });

  it('refuses a key event delivered straight to an unfocused terminal', () => {
    const { nodes, sent } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    // Aimed at the terminal's own input, bypassing every focus path this
    // component controls — the shape a stray or mis-routed event would take.
    fireEvent.keyDown(terminalInputFor('s-2'), ENTER);

    expect(sent.get('s-2')).toEqual([]);
    expect(sent.get('s-0')).toEqual([]);
  });

  it('accepts the same key event once that terminal is the focused one', async () => {
    const user = userEvent.setup();
    const { nodes, sent } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    // The paired case: proves the previous test fails for the right reason
    // rather than because the key never reached xterm at all.
    await user.click(handleFor('s-2'));
    fireEvent.keyDown(terminalInputFor('s-2'), ENTER);

    expect(sent.get('s-2')).toEqual(['\r']);
    expect(sent.get('s-0')).toEqual([]);
  });

  it('keeps unfocused terminals out of the tab order and read-only', async () => {
    const user = userEvent.setup();
    const { nodes } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    await user.click(handleFor('s-0'));

    expect(terminalInputFor('s-0').readOnly).toBe(false);
    expect(terminalInputFor('s-0').tabIndex).toBe(0);
    expect(terminalInputFor('s-1').readOnly).toBe(true);
    expect(terminalInputFor('s-1').tabIndex).toBe(-1);
  });

  it('moves focus between nodes with the arrow keys alone', async () => {
    const user = userEvent.setup();
    const { nodes } = makeFleet(4);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    handleFor('s-0').focus();
    await user.keyboard('{ArrowRight}');
    expect(handleFor('s-1')).toHaveAttribute('aria-pressed', 'true');
    expect(handleFor('s-1')).toHaveFocus();

    // Four nodes lay out two across, so down is a whole row.
    await user.keyboard('{ArrowDown}');
    expect(handleFor('s-3')).toHaveAttribute('aria-pressed', 'true');

    await user.keyboard('{Home}');
    expect(handleFor('s-0')).toHaveAttribute('aria-pressed', 'true');

    await user.keyboard('{End}');
    expect(handleFor('s-3')).toHaveAttribute('aria-pressed', 'true');
  });

  it('gives the canvas a single tab stop', () => {
    const { nodes } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-1" />);

    expect(handleFor('s-1').tabIndex).toBe(0);
    expect(handleFor('s-0').tabIndex).toBe(-1);
    expect(handleFor('s-2').tabIndex).toBe(-1);
  });

  it('leaves keys it does not own to the terminal', async () => {
    const user = userEvent.setup();
    const { nodes } = makeFleet(3);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    handleFor('s-0').focus();
    await user.keyboard('{PageDown}');

    // PageDown is not canvas navigation; focus must not have wandered.
    expect(handleFor('s-0')).toHaveAttribute('aria-pressed', 'true');
  });

  it('announces which node is focused', async () => {
    const user = userEvent.setup();
    const { nodes } = makeFleet(3);
    const { container } = render(<Harness nodes={nodes} initialFocus="s-0" />);
    const live = container.querySelector('[aria-live="polite"]');

    await user.click(handleFor('s-2'));

    expect(live).toHaveTextContent('focused, node 3 of 3.');
  });

  it('says so when nothing is focused', () => {
    const { nodes } = makeFleet(2);
    const { container } = render(<Harness nodes={nodes} initialFocus={null} />);

    expect(container.querySelector('[aria-live="polite"]')).toHaveTextContent('No session focused');
  });

  it('renders nothing but itself when there are no sessions', () => {
    render(<Harness nodes={[]} initialFocus={null} />);

    expect(screen.queryAllByRole('region')).toHaveLength(0);
  });

  it('pans on a drag that starts on the surface', () => {
    const { nodes } = makeFleet(2);
    const { container } = render(<Harness nodes={nodes} initialFocus="s-0" />);
    const surface = container.querySelector('.canvas__surface') as HTMLElement;

    fireEvent.pointerDown(surface, { button: 0 });
    expect(surface).toHaveAttribute('data-panning', 'true');

    fireEvent.pointerUp(window);
    expect(surface).toHaveAttribute('data-panning', 'false');
  });

  it('does not pan on a drag that starts inside a node', () => {
    const { nodes } = makeFleet(2);
    const { container } = render(<Harness nodes={nodes} initialFocus="s-0" />);
    const surface = container.querySelector('.canvas__surface') as HTMLElement;

    // Inside a node the gesture belongs to the terminal, which selects text.
    fireEvent.pointerDown(handleFor('s-1'), { button: 0 });

    expect(surface).toHaveAttribute('data-panning', 'false');
  });
});

describe('SessionCanvas and what a session allows', () => {
  it('never types into a stopped session, even the focused one', async () => {
    const user = userEvent.setup();
    // Idle: docs/Web_Style.md §14 says history, read-only. Focus is not a
    // second opinion on that.
    const { nodes, sent } = makeFleet(2, 'idle');
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    await user.click(handleFor('s-0'));
    fireEvent.keyDown(terminalInputFor('s-0'), ENTER);
    await user.type(terminalInputFor('s-0'), 'ls');

    expect(sent.get('s-0')).toEqual([]);
    expect(terminalInputFor('s-0').readOnly).toBe(true);
  });

  it('still shows a stopped session its history, and says it is read-only', () => {
    const { nodes, attached } = makeFleet(2, 'idle');
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    // Attached, so the transcript driver has somewhere to write.
    expect(attached.has('s-0')).toBe(true);
    expect(within(nodeFor('s-0')).getByText('read-only')).toBeInTheDocument();
  });

  it('shows why a starting session has no terminal yet', () => {
    const nodes: LiveSession[] = [{ session: session('s-0', 'provisioning', 0), driver: null }];
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    expect(within(nodeFor('s-0')).getByText(/still starting/i)).toBeInTheDocument();
    expect(nodeFor('s-0').querySelector('textarea')).toBeNull();
  });

  it('spends no attachment budget on a session with nothing to attach to', () => {
    // A dozen running sessions plus one still starting: the starting one holds
    // no terminal, so it must not push a running one out of the window.
    const { nodes, attached } = makeFleet(MAX_LIVE_NODES);
    const withPending: LiveSession[] = [
      { session: session('s-pending', 'provisioning', 99), driver: null },
      ...nodes,
    ];
    render(<Harness nodes={withPending} initialFocus="s-0" />);

    expect(attached.size).toBe(MAX_LIVE_NODES);
    expect(screen.queryByRole('status')).toBeNull();
  });
});

describe('SessionCanvas attachment budget', () => {
  const OVER = MAX_LIVE_NODES + 1;

  it('leaves the sessions past the budget detached, and says so on the node', () => {
    const { nodes, attached } = makeFleet(OVER);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    expect(attached.size).toBe(MAX_LIVE_NODES);
    expect(screen.getAllByText(/^Not attached\./)).toHaveLength(1);
  });

  it('states what the budget cost, for the status band to carry', () => {
    // §13: truncation is never silent. The count belongs in the status band,
    // which is the one part of the page that cannot be panned out of sight —
    // so the canvas reports it rather than drawing its own line beneath a
    // viewport somebody can scroll away from.
    expect(unattachedNote(OVER, MAX_LIVE_NODES)).toBe(
      `1 of ${OVER} sessions not attached, to bound what this page costs — select one to attach it`,
    );
    expect(unattachedNote(MAX_LIVE_NODES, MAX_LIVE_NODES)).toBeUndefined();
    expect(unattachedNote(3, 3)).toBeUndefined();
  });

  it('attaches a detached node when it is focused', async () => {
    const user = userEvent.setup();
    const { nodes, attached } = makeFleet(OVER);
    render(<Harness nodes={nodes} initialFocus="s-0" />);
    const last = `s-${OVER - 1}`;
    expect(attached.has(last)).toBe(false);

    await user.click(handleFor(last));

    expect(attached.has(last)).toBe(true);
    // Still within budget: something had to give way.
    expect(attached.size).toBe(MAX_LIVE_NODES);
  });

  it('never detaches the node being typed into', async () => {
    const user = userEvent.setup();
    const { nodes, attached, sent } = makeFleet(OVER);
    render(<Harness nodes={nodes} initialFocus="s-0" />);
    const last = `s-${OVER - 1}`;

    await user.click(handleFor(last));
    await user.type(terminalInputFor(last), 'z');

    expect(attached.has(last)).toBe(true);
    expect(sent.get(last)).toEqual(['z']);
  });

  it('attaches the focused node even when the history has no room left for it', () => {
    /*
     * The budget is spent from the history the caller supplies, and a caller
     * can hand over one that does not lead with the focused node — a session
     * removed from the list, a restored history, a route that forgot to record
     * a selection. §15's promise is about this component, so it holds anyway.
     */
    const { nodes, attached } = makeFleet(OVER);
    const withoutFocused = nodes.slice(1).map((node) => node.session.id);

    render(
      <SessionCanvas
        nodes={nodes}
        focusedId="s-0"
        focusHistory={withoutFocused}
        onFocus={() => undefined}
      />,
    );

    expect(attached.has('s-0')).toBe(true);
    expect(attached.size).toBe(MAX_LIVE_NODES);
  });

  it('evicts the node that has gone longest without focus', async () => {
    const user = userEvent.setup();
    const { nodes, attached } = makeFleet(OVER);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    // s-0 is the most recently focused, so touching the overflow node must
    // cost the one nothing has been done to for longest.
    await user.click(handleFor(`s-${OVER - 1}`));

    expect(attached.has('s-0')).toBe(true);
    expect(attached.has(`s-${OVER - 1}`)).toBe(true);
  });
});

describe('SessionCanvas caret placement', () => {
  it('parks the caret on the handle when the gesture lands on the chrome', async () => {
    const user = userEvent.setup();
    const { nodes, sent } = makeFleet(2);
    render(<Harness nodes={nodes} initialFocus="s-0" />);
    terminalInputFor('s-0').focus();

    await user.click(handleFor('s-1'));

    expect(handleFor('s-1')).toHaveFocus();
    // And the terminal the caret just left has gone quiet, rather than the
    // keyboard appearing dead because the caret sits in an ignored textarea.
    await user.keyboard('q');
    expect(sent.get('s-0')).toEqual([]);
  });

  it('puts the caret in the terminal when the gesture lands on it', async () => {
    const user = userEvent.setup();
    const { nodes, sent } = makeFleet(2);
    render(<Harness nodes={nodes} initialFocus="s-0" />);

    const input = terminalInputFor('s-1');
    await user.click(input);

    // One click focuses the node and leaves the caret where typing works.
    expect(handleFor('s-1')).toHaveAttribute('aria-pressed', 'true');
    expect(input).toHaveFocus();
    await user.keyboard('w');
    expect(sent.get('s-1')).toEqual(['w']);
    expect(sent.get('s-0')).toEqual([]);
  });

  it('falls back to the handle when a detached node is clicked', async () => {
    const user = userEvent.setup();
    const { nodes } = makeFleet(MAX_LIVE_NODES + 1);
    render(<Harness nodes={nodes} initialFocus="s-0" />);
    const last = `s-${MAX_LIVE_NODES}`;

    await user.click(screen.getByText(/^Not attached\./));

    expect(handleFor(last)).toHaveFocus();
    expect(handleFor(last)).toHaveAttribute('aria-pressed', 'true');
  });
});

describe('SessionCanvas driver identity', () => {
  it('does not rebuild a terminal when focus moves', async () => {
    const user = userEvent.setup();
    const attach = vi.fn();
    const driver: TerminalDriver = { attach, send: vi.fn(), resize: vi.fn(), detach: vi.fn() };
    const nodes: LiveSession[] = ['s-0', 's-1'].map((id, index) => ({
      session: session(id, 'running', index),
      driver,
    }));
    render(<Harness nodes={nodes} initialFocus="s-0" />);
    const initial = attach.mock.calls.length;

    await user.click(handleFor('s-1'));
    await user.click(handleFor('s-0'));

    // Re-attaching on every focus change would clear the scrollback and, with
    // a real transport, reconnect the socket.
    expect(attach.mock.calls.length).toBe(initial);
  });
});
