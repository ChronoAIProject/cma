import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { isWaitingForAnswer, planeStateFor } from './planeState';
import { abilitiesFor } from '../sessions/availability';
import type { Session, SessionLifecycle } from '../sessions/types';
import { NO_CAPABILITIES } from '../agui';

/**
 * Session states on the conversation plane — `Web_Style.md` §11 and §18.6.
 *
 * @vitest-environment node
 */

function session(lifecycle: SessionLifecycle, extra: Partial<Session> = {}): Session {
  return {
    id: 'sesn_1',
    kernel: 'codex',
    lifecycle,
    createdAt: Date.parse('2026-08-18T08:53:00Z'),
    capabilities: NO_CAPABILITIES,
    ...extra,
  };
}

const EVERY_STATE: readonly SessionLifecycle[] = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
];

describe('rotating', () => {
  it('refuses input', () => {
    // `API_Overview.md` §2 instructs rather than describes: "Clients must
    // refuse input while it lasts — a keystroke sent now reaches a sandbox
    // being torn down, so it would be accepted and then lost." CLAUDE.md §5:
    // accepting input that would be lost with the outgoing pod is worse than
    // briefly refusing it.
    expect(planeStateFor(session('rotating')).interactive).toBe(false);
  });

  it('says why, in the owner terms', () => {
    // Refusing is the requirement; saying why is the design.
    const reason = planeStateFor(session('rotating')).reason ?? '';
    expect(reason).toMatch(/being moved/i);
    expect(reason).toMatch(/cannot take input/i);
  });

  it('is not a failure', () => {
    expect(planeStateFor(session('rotating')).failed).toBe(false);
  });
});

describe('idle and recovering', () => {
  it('draws neither as a failure', () => {
    // §11: `idle` is a session waiting for its owner and `recovering` with
    // `runtime_absent` is normal during an idle-to-revive cycle. Drawing
    // either as an error trains owners to ignore real ones.
    expect(planeStateFor(session('idle')).failed).toBe(false);
    expect(planeStateFor(session('terminated')).failed).toBe(false);
  });

  it('draws a session that ended by failing as one', () => {
    // The one case that is a failure, and the reason the distinction exists.
    expect(planeStateFor(session('terminated', { stopReason: 'error' })).failed).toBe(true);
  });

  it('treats a session parked on an approval as the most alive a session gets', () => {
    // §11's table: idle is read-only on this plane *unless it is waiting on an
    // approval*, which is the one case where an idle session has a control to
    // offer.
    expect(isWaitingForAnswer(session('idle'), true)).toBe(true);
    expect(isWaitingForAnswer(session('idle'), false)).toBe(false);
    expect(isWaitingForAnswer(session('running'), true)).toBe(false);
  });

  it('says the agent is waiting, never that the session stopped', () => {
    // The live defect §11's wording change fixes: a session parked on
    // `requires_action` is `idle`, and the composer used to say "This session
    // is stopped. Revive it to carry on." while an answerable approval sat
    // directly above it.
    const parked = planeStateFor(session('idle'), true);
    expect(parked.waiting).toBe(true);
    expect(parked.interactive).toBe(false);
    expect(parked.reason).toMatch(/waiting for your answer/i);
    // It never claims the session is stopped, and never offers a revive.
    expect(parked.reason).not.toMatch(/session is stopped/i);
    expect(parked.reason).not.toMatch(/revive/i);
    expect(parked.failed).toBe(false);

    // A genuinely idle session is unchanged.
    const idle = planeStateFor(session('idle'), false);
    expect(idle.waiting).toBe(false);
    expect(idle.reason).toMatch(/stopped/i);
  });
});

describe('the table and the implementation agree', () => {
  it('says why for every state that cannot take input', () => {
    // §11: "a session that cannot be typed into always says why."
    for (const lifecycle of EVERY_STATE) {
      const state = planeStateFor(session(lifecycle));
      if (!state.interactive) {
        expect(state.reason, lifecycle).toBeDefined();
        expect(state.reason, lifecycle).not.toBe('');
      }
    }
  });

  it('reads one rule rather than restating it', () => {
    // §11's table grew a column rather than a twin, so this reads
    // `abilitiesFor` rather than keeping a second copy — a second copy is a
    // second place for the planes to disagree about what a state means.
    for (const lifecycle of EVERY_STATE) {
      if (lifecycle === 'rotating') {
        // The one state this plane says more about than the terminal does.
        continue;
      }
      expect(planeStateFor(session(lifecycle)).interactive, lifecycle).toBe(
        abilitiesFor(session(lifecycle)).interactive,
      );
    }

    const source = readFileSync(resolve(process.cwd(), 'src/chat/planeState.ts'), 'utf8');
    expect(source).toContain('abilitiesFor');
  });

  it('line for line, offers input only where the §11 table allows it', () => {
    // The table: provisioning and rotating refuse, ready and running are live,
    // idle and terminated are read-only.
    const allowed: Record<SessionLifecycle, boolean> = {
      provisioning: false,
      ready: true,
      running: true,
      rotating: false,
      idle: false,
      terminated: false,
    };
    for (const lifecycle of EVERY_STATE) {
      expect(planeStateFor(session(lifecycle)).interactive, lifecycle).toBe(allowed[lifecycle]);
    }
  });
});
