import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import { resolveConfig, type ResponseMetadata } from '../src/config.js';
import { readConformance, toConformance } from '../src/discovery.js';
import { ProtocolError } from '../src/errors.js';

/** @vitest-environment node */

const CONFORMANCE = {
  conformsTo: ['https://spec.chrono-ai.fun/cma/1.0/conf/rest-core'],
  version: 'development',
  build_sha: 'abc488',
};

describe('anonymous deployment discovery', () => {
  it('decodes the typed route without presenting bearer or cookie credentials', async () => {
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
            'x-request-id': 'response-discovery-488',
          },
        }),
      ),
    );
    const config = resolveConfig({
      baseUrl: 'https://cma.example.test',
      cookies: true,
      credential,
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver: (metadata) => {
        observed.push(metadata);
      },
    });

    const deployment = await readConformance(config, { requestId: 'caller-discovery-488' });

    expect(deployment).toEqual({
      conformsTo: ['https://spec.chrono-ai.fun/cma/1.0/conf/rest-core'],
      version: 'development',
      buildSha: 'abc488',
    });
    expect(credential).not.toHaveBeenCalled();
    expect(observed).toEqual([{ status: 200, requestId: 'response-discovery-488' }]);
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('https://cma.example.test/api/v1/conformance');
    expect(init.credentials).toBe('omit');
    const headers = new Headers(init.headers);
    expect(headers.get('authorization')).toBeNull();
    expect(headers.get('x-request-id')).toBe('caller-discovery-488');
  });

  it('exposes discovery through the configured high-level client', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify(CONFORMANCE), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      ),
    );
    const cma = new Cma({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(cma.readConformance()).resolves.toMatchObject({ buildSha: 'abc488' });
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('rejects partial, empty, and expanded discovery documents', () => {
    for (const malformed of [
      null,
      {},
      { ...CONFORMANCE, conformsTo: [] },
      { ...CONFORMANCE, conformsTo: [''] },
      { ...CONFORMANCE, version: '' },
      { ...CONFORMANCE, build_sha: 488 },
      { ...CONFORMANCE, extra: true },
    ]) {
      expect(() => toConformance(malformed)).toThrow(ProtocolError);
    }
  });
});
