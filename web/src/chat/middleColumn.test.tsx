import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { Console } from '../Console';
import { toSession } from '../api/client';
import { ConversationSurface } from './ConversationSurface';
import type { AguiTransport, OperationTransport } from '../agui';
import type { Capability, User } from '../auth/types';

/**
 * The middle column is drawn from the session's capabilities — `Web_Style.md`
 * §18, §12 and §1.
 *
 * The join: one rail, one session header, one details sheet, and a column that
 * draws a terminal or a conversation from what the session was granted. Not a
 * third view, and no router.
 */

const CAPABILITIES: readonly Capability[] = ['create_session', 'use_own_sessions'];
const user: User = { id: 'sub_test', handle: 'tester', capabilities: CAPABILITIES };

/** A session payload as the control plane sends it. */
function payload(id: string, planes: readonly string[]) {
  return {
    id,
    status: 'running',
    kernel: 'codex',
    created_at: '2026-08-18T08:53:00Z',
    conforms_to: [
      'https://spec.chrono-ai.fun/cma/1.0/conf/core',
      ...planes.map((name) => `https://spec.chrono-ai.fun/cma/1.0/conf/${name}`),
    ],
  };
}

describe('reading a plane off a session', () => {
  it('is the session own answer, not a preference', () => {
    expect(toSession(payload('a', ['conversation'])).plane).toBe('conversation');
    expect(toSession(payload('b', ['terminal'])).plane).toBe('terminal');
  });

  it('falls back to the terminal when the control plane did not say', () => {
    // What every session was before the conversation plane existed, and the
    // safe direction: a terminal drawn for a conversation session shows an
    // empty screen, and a conversation drawn for a terminal session shows
    // nothing at all.
    expect(toSession(payload('c', [])).plane).toBeUndefined();
  });

  it('reads the granted set once, so components ask rather than try', () => {
    const session = toSession(payload('d', ['conversation', 'usage']));
    expect(session.capabilities.conversation).toBe(true);
    expect(session.capabilities.usage).toBe(true);
    expect(session.capabilities.steer).toBe(false);
  });
});

describe('the frame does not grow a third view', () => {
  it('still admits exactly two, and still carries no router', async () => {
    // §1, and this issue's own line: "this is not a third view and must not
    // become one." The frame is not navigation and the app has no router.
    const { container } = render(<Console user={user} onSignOut={() => {}} />);
    await waitFor(() => {
      expect(container.querySelector('.frame')).not.toBeNull();
    });

    // Operations is the only other view there is, and this caller cannot
    // reach it — so there is nowhere to go from here at all.
    expect(screen.queryByRole('button', { name: 'Operations' })).toBeNull();
  });

  it('draws one sidebar, one rail and one session header whatever the plane', async () => {
    const { container } = render(<Console user={user} onSignOut={() => {}} />);
    await waitFor(() => {
      expect(container.querySelector('.frame')).not.toBeNull();
    });

    expect(container.querySelectorAll('.sidebar')).toHaveLength(1);
    expect(container.querySelectorAll('.rail')).toHaveLength(1);
    expect(container.querySelectorAll('.sessionheader')).toHaveLength(1);
  });
});

describe('runtime loss in the reading column', () => {
  it('appears above the refusal it explains and above the message stream', async () => {
    const person = userEvent.setup();
    const session = toSession(payload('sesn_lost', ['conversation']));
    const transport: AguiTransport = {
      open: (_session, onFrame) => {
        onFrame({
          type: 'STATE_DELTA',
          seq: 8,
          threadId: 'sesn_lost',
          runId: 'run_1',
          delta: [
            {
              op: 'add',
              path: '/runtimeLosses/-',
              value: {
                runtime: 'runtime-gone',
                cause: 'failed',
                noticedAt: '2026-08-23T04:12:00Z',
              },
            },
          ],
        });
        return () => {};
      },
      history: () => Promise.resolve([]),
    };
    const operations: OperationTransport = {
      send: () =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              code: 'capability_unsupported',
              detail: 'capability not supported: conversation',
            }),
            { status: 422, headers: { 'content-type': 'application/problem+json' } },
          ),
        ),
    };
    render(
      <ConversationSurface
        session={session}
        capabilities={session.capabilities}
        transport={transport}
        operations={operations}
      />,
    );

    const loss = await screen.findByRole('status');
    await person.type(screen.getByRole('textbox', { name: /message/i }), 'carry on');
    await person.click(screen.getByRole('button', { name: /send message/i }));
    const refusal = await screen.findByRole('alert');
    const stream = screen.getByRole('log', { name: 'Conversation' });

    expect(loss.compareDocumentPosition(refusal) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(loss.compareDocumentPosition(stream) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  });
});
