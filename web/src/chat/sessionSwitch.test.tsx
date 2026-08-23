import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Console } from '../Console';
import type { Capability, User } from '../auth/types';

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
const SESSIONS = ['sesn_alpha', 'sesn_beta'].map((id, index) => ({
  id,
  status: 'running',
  kernel: 'codex',
  cols: 120,
  rows: 32,
  last_seq: 3,
  created_at: new Date(Date.UTC(2026, 7, 20, 6 - index, 0)).toISOString(),
  conforms_to: [
    'https://spec.chrono-ai.fun/cma/1.0/conf/core',
    'https://spec.chrono-ai.fun/cma/1.0/conf/conversation',
  ],
}));

/**
 * A stream that opens and says nothing.
 *
 * `addEventListener` is not decoration: the AG-UI wire subscribes by event
 * name for every kind this build knows (`wire.ts`), so a stub carrying only
 * `onmessage` throws the moment a conversation surface mounts — and throws
 * outside any assertion, which is an unhandled error rather than a failure.
 */
class QuietEventSource {
  static readonly CLOSED = 2;
  readyState = 1;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  addEventListener(): void {}
  removeEventListener(): void {}
  close(): void {
    this.readyState = QuietEventSource.CLOSED;
  }
}

beforeEach(() => {
  vi.stubGlobal('EventSource', QuietEventSource);
  vi.stubGlobal(
    'fetch',
    vi.fn((input: RequestInfo | URL) => {
      const url = String(input);
      const json = (body: unknown) =>
        Promise.resolve({ ok: true, status: 200, json: async () => body } as Response);
      if (url.endsWith('/api/v1/sessions')) return json({ sessions: SESSIONS });
      if (url.includes('/api/v1/kernels')) return json({ kernels: [] });
      if (url.includes('/environment-profiles')) return json({ profiles: [] });
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
