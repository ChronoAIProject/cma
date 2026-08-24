import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { resolveConfig, type ResolvedConfig } from '../src/config.js';
import {
  CapabilityRefused,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  ValidationError,
} from '../src/errors.js';
import {
  getUsage,
  interrupt,
  resizeTerminal,
  resolveApproval,
  screenSnapshot,
  sendInput,
  steer,
} from '../src/operations.js';

/** @vitest-environment node */

function fixture(name: string): unknown {
  return JSON.parse(readFileSync(new URL(`fixtures/${name}`, import.meta.url), 'utf8')) as unknown;
}

function answering(
  body: unknown = undefined,
  status = 200,
): {
  readonly config: ResolvedConfig;
  readonly fetch: ReturnType<typeof vi.fn>;
} {
  const fetch = vi.fn(() =>
    Promise.resolve(
      new Response(body === undefined ? null : JSON.stringify(body), {
        status,
        headers: { 'Content-Type': 'application/json' },
      }),
    ),
  );
  return {
    config: resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch, cookies: false }),
    fetch,
  };
}

function requestOf(fetch: ReturnType<typeof vi.fn>, index = 0): [string, RequestInit] {
  return fetch.mock.calls[index] as unknown as [string, RequestInit];
}

describe('session write operations', () => {
  it('sends text input with the server fixture field names', async () => {
    const { config, fetch } = answering(undefined, 202);

    await sendInput(config, 'sesn/one', { text: 'Only inspect src/auth.', newline: false });

    const [url, init] = requestOf(fetch);
    expect(url).toBe('/api/v1/sessions/sesn%2Fone/input');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual(fixture('input_text_request.json'));
    expect(init.credentials).toBe('omit');
  });

  it('encodes terminal bytes without a browser or Node-only base64 global', async () => {
    const { config, fetch } = answering(undefined, 202);

    await sendInput(config, 'sesn_fixture', { bytes: new Uint8Array([0x1b, 0x0b, 0x42]) });
    await sendInput(config, 'sesn_fixture', { bytes: new Uint8Array([1]) });
    await sendInput(config, 'sesn_fixture', { bytes: new Uint8Array([1, 2]) });

    expect(JSON.parse(String(requestOf(fetch, 0)[1].body))).toEqual(
      fixture('input_bytes_request.json'),
    );
    expect(JSON.parse(String(requestOf(fetch, 1)[1].body))).toEqual({ bytes_b64: 'AQ==' });
    expect(JSON.parse(String(requestOf(fetch, 2)[1].body))).toEqual({ bytes_b64: 'AQI=' });
  });

  it('resolves an approval at its encoded request address', async () => {
    const { config, fetch } = answering(undefined, 202);

    await resolveApproval(config, 'sesn/one', 'req/two', 'abort');

    const [url, init] = requestOf(fetch);
    expect(url).toBe('/api/v1/sessions/sesn%2Fone/approvals/req%2Ftwo');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual(fixture('approval_request.json'));
  });

  it('interrupts and steers through distinct operation routes', async () => {
    const { config, fetch } = answering(undefined, 202);

    await interrupt(config, 'sesn one');
    await steer(config, 'sesn one', 'Focus on the failing assertion.');

    expect(requestOf(fetch, 0)[0]).toBe('/api/v1/sessions/sesn%20one/interrupt');
    expect(requestOf(fetch, 0)[1].body).toBeUndefined();
    expect(requestOf(fetch, 1)[0]).toBe('/api/v1/sessions/sesn%20one/steer');
    expect(JSON.parse(String(requestOf(fetch, 1)[1].body))).toEqual({
      text: 'Focus on the failing assertion.',
    });
  });

  it('sends terminal geometry exactly as the server fixture', async () => {
    const { config, fetch } = answering(undefined, 204);

    await resizeTerminal(config, 'sesn_fixture', { cols: 132, rows: 44 });

    expect(requestOf(fetch)[0]).toBe('/api/v1/sessions/sesn_fixture/resize');
    expect(JSON.parse(String(requestOf(fetch)[1].body))).toEqual(fixture('resize_request.json'));
  });

  it('keeps a typed capability refusal distinguishable from transport failure', async () => {
    const { config } = answering(fixture('capability_refused.json'), 422);

    const failure = await steer(config, 'sesn_fixture', 'continue').catch(
      (cause: unknown) => cause,
    );

    expect(failure).toBeInstanceOf(CapabilityRefused);
    expect((failure as CapabilityRefused).capability).toBe('steer');
  });

  it('requires the exact operation status and an empty successful body', async () => {
    const wrongStatus = answering(undefined, 200);
    await expect(interrupt(wrongStatus.config, 'sesn_fixture')).rejects.toBeInstanceOf(
      ProtocolError,
    );

    const body = answering({ accepted: true }, 202);
    await expect(interrupt(body.config, 'sesn_fixture')).rejects.toThrow(/carried a body/i);
  });

  it('rejects values that cannot be represented on the wire before fetching', async () => {
    const { config, fetch } = answering(undefined, 202);

    await expect(
      sendInput(config, 's', null as unknown as { text: string }),
    ).rejects.toBeInstanceOf(ValidationError);
    await expect(
      sendInput(config, 's', { bytes: [1] as unknown as Uint8Array }),
    ).rejects.toBeInstanceOf(ValidationError);
    await expect(sendInput(config, 's', { text: 3 as unknown as string })).rejects.toBeInstanceOf(
      ValidationError,
    );
    await expect(
      sendInput(config, 's', { text: 'x', newline: 'yes' as unknown as boolean }),
    ).rejects.toBeInstanceOf(ValidationError);
    await expect(resolveApproval(config, 's', 'r', 'later' as 'allow')).rejects.toBeInstanceOf(
      ValidationError,
    );
    await expect(steer(config, 's', 4 as unknown as string)).rejects.toBeInstanceOf(
      ValidationError,
    );
    await expect(resizeTerminal(config, 's', { cols: 0, rows: 24 })).rejects.toBeInstanceOf(
      ValidationError,
    );
    await expect(resizeTerminal(config, 's', { cols: 80, rows: 65_536 })).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(fetch).not.toHaveBeenCalled();
  });
});

describe('session read operations', () => {
  it('reads the rendered screen from the shared server fixture', async () => {
    const { config, fetch } = answering(fixture('screen.json'));

    await expect(screenSnapshot(config, 'sesn/one')).resolves.toEqual({
      cols: 8,
      rows: 3,
      lines: ['hello   ', 'CMA     ', '        '],
    });
    expect(requestOf(fetch)[0]).toBe('/api/v1/sessions/sesn%2Fone/screen');
    expect(requestOf(fetch)[1].method).toBe('GET');
  });

  it('reads usage without renaming or losing the optional context window', async () => {
    const { config, fetch } = answering(fixture('usage.json'));

    await expect(getUsage(config, 'sesn_fixture')).resolves.toEqual({
      inputTokens: 184_203,
      outputTokens: 9_877,
      contextUsed: 41_204,
      contextWindow: 200_000,
    });
    expect(requestOf(fetch)[0]).toBe('/api/v1/sessions/sesn_fixture/usage');

    const withoutWindow = answering({
      input_tokens: 1,
      output_tokens: 2,
      context_used: 3,
      context_window: null,
    });
    await expect(getUsage(withoutWindow.config, 's')).resolves.toEqual({
      inputTokens: 1,
      outputTokens: 2,
      contextUsed: 3,
    });
  });

  it.each([
    [null, /screen response was not an object/i],
    [{ cols: 0, rows: 1, lines: [''] }, /columns and rows/i],
    [{ cols: 2, rows: 1, lines: [3] }, /list of strings/i],
    [{ cols: 2, rows: 2, lines: ['one'] }, /declared 2 rows/i],
  ])('rejects a malformed screen response %#', async (body, expected) => {
    const { config } = answering(body);
    await expect(screenSnapshot(config, 's')).rejects.toThrow(expected);
  });

  it.each([
    [null, /usage response was not an object/i],
    [{ input_tokens: -1, output_tokens: 0, context_used: 0 }, /input_tokens/i],
    [{ input_tokens: 0, output_tokens: 0.5, context_used: 0 }, /output_tokens/i],
    [{ input_tokens: 0, output_tokens: 0, context_used: 0, context_window: -1 }, /context_window/i],
  ])('rejects a malformed usage response %#', async (body, expected) => {
    const { config } = answering(body);
    await expect(getUsage(config, 's')).rejects.toThrow(expected);
  });
});

describe('operation cancellation and deadlines', () => {
  it('propagates a pre-aborted signal through every core operation', async () => {
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });
    const controller = new AbortController();
    controller.abort('cancelled');
    const options = { signal: controller.signal };
    const calls = [
      () => sendInput(config, 's', { text: 'x' }, options),
      () => resolveApproval(config, 's', 'r', 'allow', options),
      () => interrupt(config, 's', options),
      () => steer(config, 's', 'next', options),
      () => resizeTerminal(config, 's', { cols: 80, rows: 24 }, options),
      () => screenSnapshot(config, 's', options),
      () => getUsage(config, 's', options),
    ];

    for (const call of calls) {
      await expect(call()).rejects.toBeInstanceOf(RequestAborted);
    }
    expect(fetch).not.toHaveBeenCalled();
  });

  it('aborts the underlying fetch when an operation exceeds its deadline', async () => {
    const fetch = vi.fn(
      (_url: string | URL | Request, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener(
            'abort',
            () => reject(new DOMException('aborted', 'AbortError')),
            { once: true },
          );
        }),
    );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(getUsage(config, 's', { timeoutMs: 5 })).rejects.toBeInstanceOf(TimeoutError);
  });
});
