import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import {
  AguiOperations,
  toCapabilities,
  type OperationTransport,
} from '../../src/agui/operations.js';
import {
  ApiError,
  CapabilityRefused,
  CmaError,
  ProtocolError,
  ValidationError,
} from '../../src/errors.js';

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

const accepted = () => new Response(null, { status: 202 });
const runStarted = () =>
  new Response('', { status: 200, headers: { 'content-type': 'text/event-stream' } });

function problem(
  status: number,
  code: string,
  detail: string,
  fields: Record<string, unknown> = {},
) {
  return () =>
    new Response(JSON.stringify({ code, detail, status, ...fields }), {
      status,
      headers: { 'content-type': 'application/problem+json' },
    });
}

describe('the operations', () => {
  it('opens a turn on the run endpoint', async () => {
    const { transport: t, sent } = transport(runStarted);
    await new AguiOperations('sesn_1', t).prompt('refactor auth', 'run_2');

    expect(sent[0]?.path).toBe('/api/v1/agui/run');
    expect(sent[0]?.body).toMatchObject({ threadId: 'sesn_1', runId: 'run_2' });
  });

  it('answers an approval with a new run, not a reply on the stream', async () => {
    // §4: AG-UI ends the run to ask and starts a new one to resume. An
    // approval is not a mid-stream prompt.
    const { transport: t, sent } = transport(runStarted);
    await new AguiOperations('sesn_1', t).resolveApproval('apr_7Q2', 'allow', 'run_3');

    expect(sent[0]?.path).toBe('/api/v1/agui/run');
    expect(sent[0]?.body).toMatchObject({
      threadId: 'sesn_1',
      resume: [{ interruptId: 'apr_7Q2', status: 'resolved', payload: { decision: 'allow' } }],
    });
  });

  it('refuses a decision the service does not offer', async () => {
    const { transport: t, sent } = transport(runStarted);

    await expect(
      new AguiOperations('sesn_1', t).resolveApproval('apr_1', 'allow_once', 'run_4'),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(sent).toHaveLength(0);
  });

  it('interrupts through its own route rather than by dropping the stream', async () => {
    // §5: a dropped stream is a client that stopped watching. Reading it as a
    // cancellation would end somebody's turn because their network blinked.
    const { transport: t, sent } = transport(accepted);
    await new AguiOperations('sesn_1', t).interrupt();

    expect(sent[0]?.path).toBe('/api/v1/sessions/sesn_1/interrupt');
  });

  it('releases every accepted run response immediately', async () => {
    const operations = [
      (client: AguiOperations) => client.prompt('start', 'run_prompt'),
      (client: AguiOperations) => client.steer('change course', 'run_steer'),
      (client: AguiOperations) => client.resolveApproval('apr_1', 'allow', 'run_approval'),
    ];

    for (const run of operations) {
      const cancel = vi.fn();
      const answer = () =>
        new Response(new ReadableStream({ cancel }), {
          status: 200,
          headers: { 'content-type': 'text/event-stream' },
        });
      const { transport: t } = transport(answer);

      await run(new AguiOperations('sesn_1', t));

      expect(cancel).toHaveBeenCalledOnce();
    }
  });

  it('accepts a bodiless acknowledgment and a body whose cancellation rejects', async () => {
    const bodiless = transport(
      () =>
        new Response(null, {
          status: 200,
          headers: { 'content-type': 'text/event-stream' },
        }),
    );
    await expect(
      new AguiOperations('sesn_1', bodiless.transport).prompt('start', 'run_null'),
    ).resolves.toBeUndefined();

    const rejecting = transport(
      () =>
        new Response(
          new ReadableStream({
            cancel: () => Promise.reject(new Error('connection already gone')),
          }),
          { status: 200, headers: { 'content-type': 'text/event-stream' } },
        ),
    );
    await expect(
      new AguiOperations('sesn_1', rejecting.transport).prompt('start', 'run_reject'),
    ).resolves.toBeUndefined();
  });

  it('releases a malformed successful response before reporting it', async () => {
    const cancel = vi.fn();
    const malformed = transport(
      () =>
        new Response(new ReadableStream({ cancel }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
    );

    await expect(
      new AguiOperations('sesn_1', malformed.transport).prompt('start', 'run_malformed'),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(cancel).toHaveBeenCalledOnce();
  });
});

describe('a typed refusal', () => {
  it('reaches the caller with its capability name intact', async () => {
    // `API_Overview.md` rule 4. The UI is going to show this name, so losing
    // it here would lose the only part a person can act on.
    const { transport: t } = transport(
      problem(422, 'capability_unsupported', 'the session cannot be steered', {
        capability: 'steer',
      }),
    );

    const refusal = await new AguiOperations('sesn_1', t)
      .steer('go on', 'run_5')
      .catch((cause: unknown) => cause);

    expect(refusal).toBeInstanceOf(CapabilityRefused);
    const typed = refusal as CapabilityRefused;
    expect(typed.code).toBe('capability_unsupported');
    expect(typed.capability).toBe('steer');
  });

  it('tells a permanent absence from a transient failure', async () => {
    // The distinction the whole capability model exists to preserve: only one
    // of the two is worth retrying.
    const { transport: t } = transport(problem(409, 'turn_in_progress', 'a turn is in flight'));

    const refusal = (await new AguiOperations('sesn_1', t)
      .prompt('again', 'run_6')
      .catch((cause: unknown) => cause)) as ApiError;

    expect(refusal).toBeInstanceOf(ApiError);
    expect(refusal.code).toBe('turn_in_progress');
    expect(refusal.isRetryable).toBe(false);
  });

  it('still says something true when the body is not a problem document', async () => {
    const { transport: t } = transport(() => new Response('gateway timeout', { status: 504 }));

    const refusal = await new AguiOperations('sesn_1', t)
      .prompt('x', 'run_7')
      .catch((cause: unknown) => cause);

    expect(refusal).toBeInstanceOf(ProtocolError);
    expect((refusal as Error).message).toContain('504');
  });

  it('never throws a generic failure', async () => {
    for (const answer of [
      problem(422, 'capability_unsupported', 'interrupt is unavailable', {
        capability: 'interrupt',
      }),
      problem(503, 'service_catalogue_unavailable', 'could not ask'),
      problem(409, 'session_terminated', 'ended'),
    ]) {
      const { transport: t } = transport(answer);
      const refusal = await new AguiOperations('sesn_1', t)
        .interrupt()
        .catch((cause: unknown) => cause);
      expect(refusal).toBeInstanceOf(CmaError);
    }
  });
});

describe('reading what a session was granted', () => {
  it('reads the server-emitted steering class as the steer capability', () => {
    const capabilities = toCapabilities({
      conforms_to: ['https://spec.chrono-ai.fun/cma/1.0/conf/steering'],
    });

    expect(capabilities.steer).toBe(true);
  });

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
      new URL('../../../../api/crates/layer1-public/src/rest/mod.rs', import.meta.url),
      'utf8',
    );
    const className = /SessionCapability::Resources\),\s*"(?<name>[^"]+)"/s.exec(server)?.groups
      ?.name;

    expect(className).toBe('resources');
    const conformsTo = `https://spec.chrono-ai.fun/cma/1.0/conf/${String(className)}`;
    expect(conformsTo).toBe('https://spec.chrono-ai.fun/cma/1.0/conf/resources');
    expect(toCapabilities({ conforms_to: [conformsTo] }).resources).toBe(true);
  });

  it('cross-checks every capability class against conformance_for', () => {
    const server = readFileSync(
      new URL('../../../../api/crates/layer1-public/src/rest/mod.rs', import.meta.url),
      'utf8',
    );
    const start = server.indexOf('fn conformance_for');
    const end = server.indexOf('\n}\n', start);
    expect(start).toBeGreaterThanOrEqual(0);
    expect(end).toBeGreaterThan(start);
    const implementation = server.slice(start, end);
    const emitted = [
      'core',
      'terminal',
      'conversation',
      'steering',
      'interrupt',
      'usage',
      'resources',
    ] as const;

    expect(implementation).toContain('format!("{CONF_BASE}/core")');
    for (const className of emitted.slice(1)) {
      expect(implementation, `conformance_for must emit ${className}`).toContain(`"${className}"`);
    }

    const capabilities = toCapabilities({
      conforms_to: emitted.map(
        (className) => `https://spec.chrono-ai.fun/cma/1.0/conf/${className}`,
      ),
    });
    expect(capabilities).toMatchObject({
      terminal: true,
      conversation: true,
      steer: true,
      interrupt: true,
      usage: true,
      resources: true,
    });
  });
});
