import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Console } from '../Console';
import type { Capability, User } from '../auth/types';
import { sessionPayload } from '@chronoai/cma-sdk/testing';

/**
 * Switching between sessions — one conversation at a time.
 *
 * `ConversationSurface` folds a session's events into state it holds for as
 * long as it is mounted, and says so in its own comment: *"conversation is not
 * cleared here: the surface is keyed on the session at its call sites, so a
 * different session is a different component rather than this one being
 * emptied and refilled."*
 *
 * That is a contract with whoever renders it. `SessionCanvas` holds it — its
 * nodes are keyed by session id. The list arrangement did not, so moving from
 * one session to another reused the component and folded the second session's
 * events on top of the first's: the reader watched B's turns arrive underneath
 * A's, and nothing ever removed them.
 *
 * Asserted as the surface being **replaced** rather than by comparing
 * transcripts, because that is the mechanism the contract is about — a reused
 * element is a reused fold, whatever happens to be in it at the time.
 */

const CAPABILITIES: readonly Capability[] = ['create_session', 'use_own_sessions'];
const user: User = { id: 'sub_switch', handle: 'switcher', capabilities: CAPABILITIES };

/** Two conversation sessions, as the control plane sends them. */
const SESSIONS = ['sesn_alpha', 'sesn_beta'].map((id, index) =>
  sessionPayload({
    id,
    cols: 120,
    rows: 32,
    lastSeq: 3,
    createdAt: new Date(Date.UTC(2026, 7, 20, 6 - index, 0)).toISOString(),
    grants: ['conversation'],
  }),
);

beforeEach(() => {
  vi.stubGlobal(
    'fetch',
    vi.fn((input: RequestInfo | URL) => {
      const url = String(input);
      const json = (body: unknown) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          }),
        );
      const events = (follow: boolean) =>
        Promise.resolve(
          new Response(follow ? new ReadableStream<Uint8Array>() : '', {
            status: 200,
            headers: { 'Content-Type': 'text/event-stream' },
          }),
        );
      if (url.endsWith('/api/v1/sessions')) return json({ sessions: SESSIONS });
      if (url.includes('/api/v1/kernels')) return json({ kernels: [] });
      if (url.includes('/environment-profiles')) return json([]);
      if (url.includes('/api/v1/agui/events')) return events(!url.includes('follow=false'));
      return json({});
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('moving between sessions', () => {
  it('replaces the conversation rather than folding the next one into it', async () => {
    const person = userEvent.setup();
    const { container } = render(<Console user={user} onSignOut={() => {}} />);

    const rows = await waitFor(() => {
      const found = screen.getAllByRole('tab');
      expect(found.length).toBeGreaterThanOrEqual(2);
      return found;
    });

    await person.click(rows[0]!);
    const first = await waitFor(() => {
      const surface = container.querySelector('.conversation');
      expect(surface).not.toBeNull();
      return surface;
    });

    await person.click(rows[1]!);
    await waitFor(() => {
      expect(container.querySelector('.conversation')).not.toBe(first);
    });

    // And exactly one, so the previous session's is gone rather than hidden.
    expect(container.querySelectorAll('.conversation')).toHaveLength(1);
  });
});
