import { afterEach, describe, expect, it, vi } from 'vitest';
import { listAgentImages, resolveAgentImage } from './imageCatalog';

afterEach(() => vi.unstubAllGlobals());

function page(items: readonly unknown[], next_cursor: string | null = null) {
  return new Response(JSON.stringify({ items, next_cursor }));
}

describe('Image Manager choices', () => {
  it('includes private and public images beyond the first page and preserves scope and cancellation', async () => {
    const first = { id: 'private', name: 'Private runtime', visibility: 'private' };
    const second = { id: 'public', name: 'Public runtime', visibility: 'public' };
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(page([first], 'next/id'))
      .mockResolvedValueOnce(page([second]));
    vi.stubGlobal('fetch', fetcher);
    const controller = new AbortController();
    expect(await listAgentImages(controller.signal)).toEqual([first, second]);
    expect(fetcher).toHaveBeenLastCalledWith(
      '/api/image-manager/v1/images?scope=all&after=next%2Fid',
      expect.objectContaining({ signal: controller.signal, credentials: 'same-origin' }),
    );
  });

  it('selects the newest successful immutable version across all ID-sorted pages', async () => {
    const old = {
      id: 'version-a',
      phase: 'succeeded',
      reference: 'registry/runtime@sha256:old',
      created_at_ms: 10,
    };
    const newest = {
      ...old,
      id: 'version-z',
      reference: 'registry/runtime@sha256:new',
      created_at_ms: 100,
    };
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(page([old], 'version-a'))
      .mockResolvedValueOnce(page([newest]));
    vi.stubGlobal('fetch', fetcher);
    expect(await resolveAgentImage('selected/image')).toEqual({
      id: 'selected/image',
      version: 'version-z',
      reference: newest.reference,
    });
    expect(fetcher).toHaveBeenLastCalledWith(
      '/api/image-manager/v1/images/selected%2Fimage/versions?after=version-a',
      expect.anything(),
    );
  });

  it('does not silently accept a partial catalog when a later page fails', async () => {
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValueOnce(page([{ id: 'partial' }], 'after'))
        .mockResolvedValueOnce(new Response('', { status: 503 })),
    );
    await expect(listAgentImages()).rejects.toThrow('Image Manager is unavailable (503)');
  });

  it('refuses repeated cursors and malformed catalog responses', async () => {
    const fetcher = vi.fn().mockImplementation(async () => page([], 'loop'));
    vi.stubGlobal('fetch', fetcher);
    await expect(listAgentImages()).rejects.toThrow('incomplete catalog');
    expect(fetcher).toHaveBeenCalledTimes(2);
    fetcher.mockResolvedValueOnce(new Response('{}'));
    await expect(listAgentImages()).rejects.toThrow('invalid catalog');
  });
});
