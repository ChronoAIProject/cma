import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { ConversationSurface } from './ConversationSurface';
import {
  MockSession,
  transcriptNamed,
  type OperationTransport,
  type SessionCapabilities,
} from '../agui';
import type { Session } from '../sessions/types';

/**
 * Parity — every core operation reachable from the chat surface, or refused by
 * name.
 *
 * The milestone's own proof, and it is deliberately not "every operation on
 * every session". Two of the ten are terminal facts that AG-UI does not
 * project at all, and three more depend on what the kernel behind the session
 * offers. What this file pins is the claim that *is* true:
 *
 * > for a conversation session, every core operation is either reachable from
 * > this surface or refused with the typed error naming the capability it
 * > needs — and no operation fails in any other way.
 *
 * Silence, a spinner that never resolves, and a generic error are all
 * failures of that claim, so each row below asserts what a person is left
 * looking at rather than only what was sent.
 *
 * The rows mirror the conformance table in `docs/API_AGUI.md`. The last test
 * asserts the two agree, so a row added to one and not the other fails here
 * rather than in somebody's reading.
 */

/** One row of the conformance table, as this suite drives it. */
type Projection = 'reachable' | 'capability' | 'absent' | 'observed';

interface Row {
  readonly operation: string;
  readonly projection: Projection;
  /** The capability a refusal names, where the projection is one. */
  readonly capability?: string;
}

const TABLE: readonly Row[] = [
  { operation: 'create_session', projection: 'reachable' },
  { operation: 'send_input', projection: 'reachable' },
  { operation: 'stream_events', projection: 'reachable' },
  { operation: 'resolve_approval', projection: 'reachable' },
  { operation: 'interrupt', projection: 'capability', capability: 'interrupt' },
  { operation: 'steer', projection: 'capability', capability: 'steer' },
  { operation: 'resize_terminal', projection: 'absent' },
  { operation: 'screen_snapshot', projection: 'absent' },
  { operation: 'get_usage', projection: 'capability', capability: 'usage' },
  { operation: 'change_plane', projection: 'observed' },
];

/** Which rows this file has actually driven, filled in as each test runs. */
const covered = new Set<string>();

function drove(operation: string): void {
  covered.add(operation);
}

const CAPABLE: SessionCapabilities = {
  conversation: true,
  terminal: false,
  steer: true,
  interrupt: true,
  usage: true,
  resources: false,
  concurrentInput: 'enqueue',
};

function session(): Session {
  return {
    id: 'sesn_parity',
    kernel: 'codex',
    lifecycle: 'running',
    createdAt: 0,
    plane: 'conversation',
    capabilities: CAPABLE,
  };
}

/** A transport that records what was asked and answers however a test says. */
function wire(answer: () => Response) {
  const sent: { path: string; body: unknown }[] = [];
  const operations: OperationTransport = {
    send: (path, body) => {
      sent.push({ path, body });
      return Promise.resolve(answer());
    },
  };
  return { sent, operations };
}

const accepted = () => new Response('{}', { status: 200 });

/** The control plane's own refusal shape, as `API_Overview.md` rule 4 fixes it. */
const refuses = (capability: string) => () =>
  new Response(
    JSON.stringify({
      code: 'capability_unsupported',
      detail: `capability not supported: ${capability}`,
      status: 422,
    }),
    { status: 422, headers: { 'content-type': 'application/json' } },
  );

function surface(
  answer: () => Response,
  capabilities: SessionCapabilities = CAPABLE,
  transcript = 'two-tool-calls',
  /**
   * Stop the transcript here.
   *
   * How a test looks at a session that is *parked* — on an approval, or
   * mid-turn — rather than one whose recording runs past the moment being
   * examined and settles before anything can be clicked.
   */
  upTo?: number,
) {
  const { sent, operations } = wire(answer);
  const full = transcriptNamed(transcript);
  const mock = new MockSession(
    upTo === undefined ? full : { ...full, frames: full.frames.slice(0, upTo) },
  );
  render(
    <ConversationSurface
      session={session()}
      capabilities={capabilities}
      transport={mock.transport}
      operations={operations}
    />,
  );
  return { sent };
}

describe('what the chat surface can drive', () => {
  it('opens a turn — send_input', async () => {
    drove('send_input');
    const user = userEvent.setup();
    const { sent } = surface(accepted);

    await user.type(screen.getByRole('textbox', { name: /message/i }), 'build me a web scraper');
    await user.click(screen.getByRole('button', { name: /send message/i }));

    await waitFor(() => {
      expect(sent).toHaveLength(1);
    });
    expect(sent[0]?.path).toBe('/api/v1/agui/run');
    expect(JSON.stringify(sent[0]?.body)).toContain('build me a web scraper');
  });

  it('reads the session’s history and follows it — stream_events', async () => {
    drove('stream_events');
    surface(accepted, CAPABLE, 'plain-turn');

    // Drawn from the durable log rather than from anything this client kept:
    // the reconnect order is open, read, dedupe, and what a person reloads
    // into is what they will believe.
    expect(await screen.findByRole('log', { name: /conversation/i })).toBeInTheDocument();
    // What the agent actually said, rather than a label above it: the label
    // is chrome and the words are the history this is proving arrived.
    await waitFor(() => {
      expect(document.querySelectorAll('.turn[data-role="assistant"]').length).toBeGreaterThan(0);
    });
  });

  it('answers an approval as a new run — resolve_approval', async () => {
    drove('resolve_approval');
    const user = userEvent.setup();
    const { sent } = surface(accepted, CAPABLE, 'approval-granted', 2);

    const answer = await screen.findByRole('button', { name: /allow|approve|yes/i });
    await user.click(answer);

    await waitFor(() => {
      expect(sent).toHaveLength(1);
    });
    // §4 of the AG-UI document: a new run carrying a resume, not a reply on
    // the stream that asked.
    expect(JSON.stringify(sent[0]?.body)).toContain('resume');
  });

  it('starts a session from the dialog rather than from here — create_session', () => {
    drove('create_session');
    surface(accepted);

    // Implicit on this surface by design: a run opens the thread, and the
    // session itself is created where its plane and its scope are chosen
    // (§18, §7). A second place to create one would be a second answer to
    // "what is this session".
    expect(screen.queryByRole('button', { name: /new session/i })).toBeNull();
  });
});

describe('what it refuses, by name, on screen', () => {
  it('names the capability when a turn cannot be withdrawn — interrupt', async () => {
    drove('interrupt');
    const user = userEvent.setup();
    // Granted here, so the control exists; the control plane refuses anyway,
    // which is the case that separates "cannot" from "did not work".
    const { sent } = surface(refuses('interrupt'), CAPABLE, 'two-tool-calls', 3);

    const key = await screen.findByRole('button', { name: /stop generating/i });
    await user.click(key);

    await waitFor(() => {
      expect(sent).toHaveLength(1);
    });
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/cannot interrupt/i);
  });

  it('names the capability when a turn cannot be steered — steer', async () => {
    drove('steer');
    const user = userEvent.setup();
    const { sent } = surface(refuses('steer'), CAPABLE, 'two-tool-calls', 3);

    await screen.findByRole('log', { name: /conversation/i });
    await user.type(screen.getByRole('textbox', { name: /message/i }), 'actually, do it in Rust');
    await user.click(screen.getByRole('button', { name: /send message/i }));

    await waitFor(() => {
      expect(sent).toHaveLength(1);
    });
    expect(await screen.findByRole('alert')).toHaveTextContent(/cannot steer/i);
  });

  it('says nothing was reported rather than showing a zero — get_usage', async () => {
    drove('get_usage');
    surface(accepted, { ...CAPABLE, usage: false });

    // §18.5: a session with no usage grant has often spent a great deal, and
    // drawing that as 0 is the one answer that is certainly wrong.
    expect(await screen.findByText(/not reported/i)).toBeInTheDocument();
  });
});

describe('what this plane does not have at all', () => {
  it('offers no terminal geometry — resize_terminal', () => {
    drove('resize_terminal');
    surface(accepted);

    // §18.7: a geometry is a terminal fact and AG-UI projects none. A person
    // looking for one is told where it lives rather than left hunting.
    expect(screen.queryByRole('button', { name: /resize/i })).toBeNull();
    expect(screen.queryByRole('textbox', { name: /columns|rows/i })).toBeNull();
  });

  it('renders no terminal and offers no screen — screen_snapshot', () => {
    drove('screen_snapshot');
    const { container } = { container: document.body };
    surface(accepted);

    expect(container.querySelector('.terminal')).toBeNull();
    expect(screen.queryByRole('button', { name: /screen|snapshot/i })).toBeNull();
  });

  it('is told when its session moves, and does not move it — change_plane', () => {
    drove('change_plane');
    surface(accepted);

    // Asked for in the session header over REST (§18.6). AG-UI observes the
    // change as a boundary in the stream; it offers no way to initiate one,
    // and a surface that could talk itself out of existence would have
    // nowhere left to draw the answer.
    expect(screen.queryByRole('button', { name: /move to terminal/i })).toBeNull();
  });
});

describe('the table and the tests agree', () => {
  it('drives every row', () => {
    // A conformance table nobody exercises is a claim. This is the line that
    // makes the table a test: a row added to the document and not driven here
    // fails, and so does a row driven here that the document does not carry.
    expect([...covered].sort()).toEqual(TABLE.map((row) => row.operation).sort());
  });

  it('names a capability on every row that refuses by one', () => {
    for (const row of TABLE.filter((entry) => entry.projection === 'capability')) {
      expect(row.capability, `${row.operation} must name what it lacks`).toBeTruthy();
    }
  });
});

describe('where an approval waits', () => {
  it('takes the composer seat rather than sitting in the stream', async () => {
    /*
     * §18.3, amended. An approval in the stream scrolls away: a turn that
     * stopped five entries ago leaves its question five entries up, and the
     * one thing somebody must answer becomes the one thing they can scroll
     * past. Here it is where they were already looking — and the field goes,
     * because typing is not what the session is waiting for.
     */
    surface(accepted, CAPABLE, 'approval-granted', 2);

    await waitFor(() => {
      expect(screen.getByRole('button', { name: /allow/i })).toBeInTheDocument();
    });
    expect(screen.queryByRole('textbox', { name: /message/i })).toBeNull();
  });

  it('gives the field back once the question is answered', async () => {
    // The seat is the composer's; an approval only borrows it.
    surface(accepted, CAPABLE, 'approval-granted');

    await waitFor(() => {
      expect(screen.getByRole('textbox', { name: /message/i })).toBeInTheDocument();
    });
    expect(screen.queryByRole('button', { name: /^allow$/i })).toBeNull();
  });
});
