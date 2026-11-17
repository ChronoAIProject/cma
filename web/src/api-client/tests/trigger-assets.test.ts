// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import { ApiError, ProtocolError, ValidationError, createClient } from '../index.js';

describe('anonymous Trigger assets', () => {
  it('supports SVG reads and conditional cache hits without sending credentials', async () => {
    const credential = vi.fn(() => {
      throw new Error('must remain anonymous');
    });
    let unchanged = false;
    const fetch = vi.fn(
      async () =>
        new Response(unchanged ? null : '<svg/>', {
          status: unchanged ? 304 : 200,
          headers: { etag: '"badge-one"', 'content-type': 'image/svg+xml; charset=utf-8' },
        }),
    );
    const client = createClient({
      baseUrl: 'https://contract.example',
      credential,
      cookies: true,
      fetch,
    });
    expect(
      await client.triggers.badge('pub_example', {
        label: 'try_it',
        theme: 'dark',
        placement: 'plc_example',
      }),
    ).toEqual({ status: 200, svg: '<svg/>', etag: '"badge-one"' });
    unchanged = true;
    expect(await client.triggers.badge('pub_example', { ifNoneMatch: '"badge-one"' })).toEqual({
      status: 304,
      etag: '"badge-one"',
    });
    expect(credential).not.toHaveBeenCalled();
    const calls = fetch.mock.calls as unknown as [string, RequestInit][];
    expect(new URL(calls[0]![0]).searchParams.get('placement')).toBe('plc_example');
    expect(new Headers(calls[1]![1].headers).get('if-none-match')).toBe('"badge-one"');
    for (const [, init] of calls) {
      expect(init.credentials).toBe('omit');
      expect(init.redirect).toBe('error');
      expect(new Headers(init.headers).has('authorization')).toBe(false);
    }
  });

  it('reads the embed script as text and preserves server refusals', async () => {
    const client = createClient({
      fetch: vi.fn(
        async () =>
          new Response('customElements.get("cma-trigger");', {
            headers: { 'content-type': 'application/javascript' },
          }),
      ),
    });
    expect(await client.triggers.embedScript()).toBe('customElements.get("cma-trigger");');
    const unavailable = createClient({
      fetch: vi.fn(
        async () =>
          new Response(
            JSON.stringify({ status: 503, code: 'badge_unavailable', detail: 'Unavailable' }),
            { status: 503 },
          ),
      ),
    });
    await expect(unavailable.triggers.badge('pub_example')).rejects.toBeInstanceOf(ApiError);
  });

  it('refuses wrong media, invalid UTF-8, missing cache identity and unsolicited cache hits', async () => {
    for (const reply of [
      () => new Response('<svg/>', { headers: { 'content-type': 'text/html', etag: '"one"' } }),
      () =>
        new Response(new Uint8Array([255]), {
          headers: { 'content-type': 'image/svg+xml', etag: '"one"' },
        }),
      () => new Response('<svg/>', { headers: { 'content-type': 'image/svg+xml' } }),
      () => new Response(null, { status: 304, headers: { etag: '"one"' } }),
    ]) {
      const client = createClient({ fetch: vi.fn(async () => reply()) });
      await expect(client.triggers.badge('pub_example')).rejects.toBeInstanceOf(ProtocolError);
    }
  });

  it('rejects invalid closed values and header input before transport', () => {
    const fetch = vi.fn();
    const client = createClient({ fetch });
    for (const options of [
      { theme: 'invalid' as never },
      { label: 'invalid' as never },
      { placement: '../bad' },
      { ifNoneMatch: 'bad\r\nheader' },
    ])
      expect(() => client.triggers.badge('pub_example', options)).toThrow(ValidationError);
    expect(fetch).not.toHaveBeenCalled();
  });
});
