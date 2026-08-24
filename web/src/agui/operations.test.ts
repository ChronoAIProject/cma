import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { AguiOperations, AguiRefusal, toCapabilities, type OperationTransport } from './operations';

/**
 * The operations a conversation client can perform — `docs/API_AGUI.md`.
 *
 * @vitest-environment node
 */

/** A transport that records what was sent and answers what a test chose. */
function transport(answer: () => Response) {
  const sent: { path: string; body: unknown }[] = [];
  const t: OperationTransport = {
    send: (path, body) => {
      sent.push({ path, body });
      return Promise.resolve(answer());
    },
  };
  return { transport: t, sent };
}

const ok = () => new Response(null, { status: 202 });

function problem(status: number, code: string, detail: string) {
  return () =>
    new Response(JSON.stringify({ code, detail, status }), {
      status,
      headers: { 'content-type': 'application/problem+json' },
    });
}

describe('the operations', () => {
  it('opens a turn on the run endpoint', async () => {
    const { transport: t, sent } = transport(ok);
    await new AguiOperations('sesn_1', t).prompt('refactor auth', 'run_2');

    expect(sent[0]?.path).toBe('/api/v1/agui/run');
    expect(sent[0]?.body).toMatchObject({ threadId: 'sesn_1', runId: 'run_2' });
  });

  it('answers an approval with a new run, not a reply on the stream', async () => {
    // §4: AG-UI ends the run to ask and starts a new one to resume. An
    // approval is not a mid-stream prompt.
    const { transport: t, sent } = transport(ok);
    await new AguiOperations('sesn_1', t).resolveApproval('apr_7Q2', 'allow', 'run_3');

    expect(sent[0]?.path).toBe('/api/v1/agui/run');
    expect(sent[0]?.body).toMatchObject({
      threadId: 'sesn_1',
      resume: [{ interruptId: 'apr_7Q2', status: 'resolved', payload: { decision: 'allow' } }],
    });
  });

  it('sends the decision the kernel offered, unchanged', async () => {
    // A value this client invented would be read as a refusal — the one
    // direction the service safely fails in — so it must not invent one.
    const { transport: t, sent } = transport(ok);
    await new AguiOperations('sesn_1', t).resolveApproval('apr_1', 'allow_once', 'run_4');

    expect(sent[0]?.body).toMatchObject({
      resume: [{ payload: { decision: 'allow_once' } }],
    });
  });

  it('interrupts through its own route rather than by dropping the stream', async () => {
    // §5: a dropped stream is a client that stopped watching. Reading it as a
    // cancellation would end somebody's turn because their network blinked.
    const { transport: t, sent } = transport(ok);
    await new AguiOperations('sesn_1', t).interrupt();

    expect(sent[0]?.path).toBe('/api/v1/sessions/sesn_1/interrupt');
  });
});

describe('a typed refusal', () => {
  it('reaches the caller with its capability name intact', async () => {
    // `API_Overview.md` rule 4. The UI is going to show this name, so losing
    // it here would lose the only part a person can act on.
    const { transport: t } = transport(
      problem(422, 'capability_unsupported', 'capability not supported: steer'),
    );

    const refusal = await new AguiOperations('sesn_1', t)
      .steer('go on', 'run_5')
      .catch((cause: unknown) => cause);

    expect(refusal).toBeInstanceOf(AguiRefusal);
    const typed = refusal as AguiRefusal;
    expect(typed.code).toBe('capability_unsupported');
    expect(typed.capability).toBe('steer');
    expect(typed.isCapability).toBe(true);
  });

  it('tells a permanent absence from a transient failure', async () => {
    // The distinction the whole capability model exists to preserve: only one
    // of the two is worth retrying.
    const { transport: t } = transport(problem(409, 'turn_in_progress', 'a turn is in flight'));

    const refusal = (await new AguiOperations('sesn_1', t)
      .prompt('again', 'run_6')
      .catch((cause: unknown) => cause)) as AguiRefusal;

    expect(refusal.code).toBe('turn_in_progress');
    expect(refusal.isCapability).toBe(false);
  });

  it('still says something true when the body is not a problem document', async () => {
    const { transport: t } = transport(() => new Response('gateway timeout', { status: 504 }));

    const refusal = (await new AguiOperations('sesn_1', t)
      .prompt('x', 'run_7')
      .catch((cause: unknown) => cause)) as AguiRefusal;

    expect(refusal.code).toBe('http_504');
    expect(refusal.message).toContain('504');
  });

  it('never throws a generic failure', async () => {
    for (const answer of [
      problem(422, 'capability_unsupported', 'capability not supported: interrupt'),
      problem(503, 'service_catalogue_unavailable', 'could not ask'),
      problem(409, 'session_terminated', 'ended'),
    ]) {
      const { transport: t } = transport(answer);
      const refusal = await new AguiOperations('sesn_1', t)
        .interrupt()
        .catch((cause: unknown) => cause);
      expect(refusal).toBeInstanceOf(AguiRefusal);
    }
  });
});

describe('reading what a session was granted', () => {
  it('lets a component ask rather than try', () => {
    // #116: read the granted set once and expose it, so components ask "may
    // this session steer" instead of trying and handling a refusal.
    const capabilities = toCapabilities({
      conforms_to: [
        'https://spec.chrono-ai.fun/cma/1.0/conf/core',
        'https://spec.chrono-ai.fun/cma/1.0/conf/conversation',
        'https://spec.chrono-ai.fun/cma/1.0/conf/usage',
      ],
      concurrent_input: 'enqueue',
    });

    expect(capabilities.conversation).toBe(true);
    expect(capabilities.usage).toBe(true);
    expect(capabilities.resources).toBe(false);
    expect(capabilities.steer).toBe(false);
    expect(capabilities.terminal).toBe(false);
    expect(capabilities.concurrentInput).toBe('enqueue');
  });

  it('fails closed on anything it was not clearly told', () => {
    // The worst outcome is a control that is not offered; the alternative is
    // offering one the API will refuse.
    for (const raw of [null, undefined, {}, { conforms_to: 'not an array' }]) {
      const capabilities = toCapabilities(raw);
      expect(capabilities.conversation).toBe(false);
      expect(capabilities.steer).toBe(false);
      expect(capabilities.usage).toBe(false);
      expect(capabilities.resources).toBe(false);
    }
  });

  it('reads an unrecognised concurrency policy as the strictest one', () => {
    // `reject` refuses a second turn; anything looser could send one the
    // session then loses.
    expect(toCapabilities({ concurrent_input: 'something_new' }).concurrentInput).toBe('reject');
  });

  it("pins the server and client's exact resource conformance spelling", () => {
    const server = readFileSync(
      new URL('../../../api/crates/layer1-public/src/rest/mod.rs', import.meta.url),
      'utf8',
    );
    const className = /SessionCapability::Resources\),\s*"(?<name>[^"]+)"/s.exec(server)?.groups
      ?.name;

    expect(className).toBe('resources');
    const conformsTo = `https://spec.chrono-ai.fun/cma/1.0/conf/${String(className)}`;
    expect(conformsTo).toBe('https://spec.chrono-ai.fun/cma/1.0/conf/resources');
    expect(toCapabilities({ conforms_to: [conformsTo] }).resources).toBe(true);
  });
});
