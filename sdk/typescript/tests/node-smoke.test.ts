import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { createClient } from '../src/index.js';

/** @vitest-environment node */

function fixtureText(name: string): string {
  return readFileSync(new URL(`fixtures/${name}`, import.meta.url), 'utf8');
}

function fixture(name: string): unknown {
  return JSON.parse(fixtureText(name)) as unknown;
}

describe('Node-only create, stream, interrupt smoke', () => {
  it('drives one session without any browser global', async () => {
    expect('window' in globalThis).toBe(false);
    expect('document' in globalThis).toBe(false);

    const calls: { readonly url: string; readonly init: RequestInit }[] = [];
    const event = fixture('event.json') as { readonly seq: number };
    const fetch = vi.fn((input: string | URL | Request, init?: RequestInit) => {
      const url = String(input);
      const request = { url, init: init ?? {} };
      calls.push(request);

      if (url === 'https://cma.example.test/api/v1/sessions' && request.init.method === 'POST') {
        return Promise.resolve(
          new Response(fixtureText('session.json'), {
            status: 201,
            headers: { 'Content-Type': 'application/json' },
          }),
        );
      }
      if (url.endsWith('/sessions/sesn_fixture/events?after=7')) {
        return Promise.resolve(
          new Response(
            `id: ${String(event.seq)}\ndata: ${JSON.stringify(fixture('event.json'))}\n\n`,
            {
              status: 200,
              headers: { 'Content-Type': 'text/event-stream' },
            },
          ),
        );
      }
      if (url.endsWith('/sessions/sesn_fixture/interrupt')) {
        return Promise.resolve(new Response(null, { status: 202 }));
      }
      return Promise.reject(new Error(`unexpected smoke request: ${url}`));
    });
    let credentialReads = 0;
    const cma = createClient({
      baseUrl: 'https://cma.example.test',
      cookies: false,
      credential: () => `token-${String(++credentialReads)}`,
      fetch: fetch as unknown as typeof globalThis.fetch,
    });

    const session = await cma.createSession({
      cols: 120,
      rows: 40,
      environmentProfile: 'envp_fixture',
      quotedRateCard: 'rate_fixture',
    });
    const received = [];
    for await (const frame of cma.streamEvents(session.id, { startingAfter: session.lastSeq })) {
      received.push(frame);
      break;
    }
    await cma.interrupt(session.id);

    expect(session).toMatchObject({ id: 'sesn_fixture', lastSeq: 7 });
    expect(received).toEqual([
      {
        seq: 8,
        sessionId: 'sesn_fixture',
        event: {
          type: 'approval_requested',
          request_id: 'req_fixture',
          action: 'Run the release command',
          detail: 'This command writes a release tag',
        },
      },
    ]);
    expect(JSON.parse(String(calls[0]?.init.body))).toEqual(fixture('create_session_request.json'));
    expect(calls.map(({ init }) => init.credentials)).toEqual(['omit', 'omit', 'omit']);
    expect(calls.map(({ init }) => new Headers(init.headers).get('authorization'))).toEqual([
      'Bearer token-1',
      'Bearer token-2',
      'Bearer token-3',
    ]);
    expect(new Headers(calls[1]?.init.headers).get('last-event-id')).toBe('7');
    expect(calls[2]).toMatchObject({
      url: 'https://cma.example.test/api/v1/sessions/sesn_fixture/interrupt',
      init: { method: 'POST' },
    });
  });
});
