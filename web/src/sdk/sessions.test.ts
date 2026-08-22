import { describe, expect, it, vi } from 'vitest';
import { resolveConfig } from './config';
import { readAllowance } from './sessions';

/** @vitest-environment node */

function answering(body: unknown) {
  const fetch = vi.fn(() =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: () => Promise.resolve(body),
    } as Response),
  );
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls: fetch };
}

describe('the live-session allowance', () => {
  it('reads the caller-scoped advisory response', async () => {
    const { fetch, calls } = answering({ limit: 3, live: 2, exempt: false });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      limit: 3,
      live: 2,
      exempt: false,
    });
    expect(calls).toHaveBeenCalledWith(
      '/api/v1/sessions/allowance',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('keeps an unconfigured limit absent', async () => {
    const { fetch } = answering({ live: 7, exempt: true });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      live: 7,
      exempt: true,
    });
  });

  it('treats malformed advisory fields as permissive defaults', async () => {
    const { fetch } = answering({ limit: 0, live: 'many', exempt: 'yes' });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      live: 0,
      exempt: false,
    });
  });
});
