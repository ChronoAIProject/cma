import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import { resolveConfig, type ResponseMetadata } from '../src/config.js';
import { readConformance, toConformance } from '../src/discovery.js';
import { ProtocolError } from '../src/errors.js';

/** @vitest-environment node */

const CONFORMANCE = {
  service: 'cma',
  status: 'rebuilding',
  interfaces: [],
};

describe('anonymous deployment discovery', () => {
  it('uses the public route without presenting bearer or cookie credentials', async () => {
    const credential = vi.fn(() => {
      throw new Error('anonymous discovery must not resolve a bearer');
    });
    const observed: ResponseMetadata[] = [];
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify(CONFORMANCE), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            'x-request-id': 'response-discovery-801',
          },
        }),
      ),
    );
    const config = resolveConfig({
      baseUrl: 'https://cma.example.test',
      credential,
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver: (metadata) => {
        observed.push(metadata);
      },
    });

    await expect(readConformance(config, { requestId: 'caller-discovery-801' })).resolves.toEqual(
      CONFORMANCE,
    );
    expect(credential).not.toHaveBeenCalled();
    expect(observed).toEqual([{ status: 200, requestId: 'response-discovery-801' }]);

    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('https://cma.example.test/api/v1/conformance');
    expect(init.credentials).toBe('omit');
    expect(new Headers(init.headers).get('authorization')).toBeNull();
    expect(new Headers(init.headers).get('x-request-id')).toBe('caller-discovery-801');
  });

  it('is available through the configured client', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify(CONFORMANCE), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      ),
    );
    const client = new Cma({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(client.readConformance()).resolves.toEqual(CONFORMANCE);
    expect(client.config.fetch).toBe(fetch);
  });

  it('requires the canonical success status', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(new Response(JSON.stringify(CONFORMANCE), { status: 201 })),
    );

    await expect(
      readConformance(resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch })),
    ).rejects.toThrow(/expected HTTP 200/i);
  });

  it('rejects partial, malformed, and expanded documents', () => {
    for (const malformed of [
      null,
      [],
      {},
      { ...CONFORMANCE, service: '' },
      { ...CONFORMANCE, status: 801 },
      { ...CONFORMANCE, interfaces: 'none' },
      { ...CONFORMANCE, interfaces: [''] },
      { ...CONFORMANCE, extra: true },
    ]) {
      expect(() => toConformance(malformed)).toThrow(ProtocolError);
    }
  });
});
