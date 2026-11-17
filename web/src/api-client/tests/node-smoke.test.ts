// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import { createClient } from '../index.js';

/** @vitest-environment node */

describe('Node discovery smoke', () => {
  it('reads deployment state without browser globals', async () => {
    expect('window' in globalThis).toBe(false);
    expect('document' in globalThis).toBe(false);

    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify({ service: 'cma', status: 'rebuilding', interfaces: [] }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      ),
    );
    const client = createClient({
      baseUrl: 'https://cma.example.test',
      fetch: fetch as unknown as typeof globalThis.fetch,
    });

    await expect(client.readConformance()).resolves.toEqual({
      service: 'cma',
      status: 'rebuilding',
      interfaces: [],
    });
    expect(fetch).toHaveBeenCalledOnce();
  });
});
