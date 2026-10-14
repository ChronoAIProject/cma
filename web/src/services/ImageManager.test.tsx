import userEvent from '@testing-library/user-event';
import { chooseOptionValue } from '../test/select';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ImageManager } from './ImageManager';
import type { ServiceSession } from './client';

afterEach(() => vi.unstubAllGlobals());

const session: ServiceSession = {
  authenticated: true,
  subject: 'owner',
  permissions: ['cma-im:admin'],
  expires_at: Math.floor(Date.now() / 1000) + 3600,
};
const image = {
  id: 'image-one',
  owner: 'owner',
  name: 'My runtime',
  description: 'Runtime image',
  shares: ['trusted-account'],
  visibility: 'private',
  retired: false,
  revision: 4,
};
const page = (items: unknown[], next_cursor: string | null = null) =>
  new Response(JSON.stringify({ items, next_cursor }));

it('publishes and withdraws an owned image with the current revision and preserves shares', async () => {
  let current = { ...image };
  const writes: RequestInit[] = [];
  const fetcher = vi.fn(async (url: string, init: RequestInit = {}) => {
    if (url.endsWith('/visibility')) {
      writes.push(init);
      current = { ...current, ...JSON.parse(init.body as string), revision: current.revision + 1 };
      return new Response(JSON.stringify(current));
    }
    return url.includes('/builds') ? page([]) : page([current]);
  });
  vi.stubGlobal('fetch', fetcher);
  render(<ImageManager session={session} denied={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Open image' }));
  fireEvent.click(screen.getByText('Sharing'));
  fireEvent.click(screen.getByRole('button', { name: 'Make public' }));
  await screen.findByRole('button', { name: 'Make private' });
  expect(writes[0]).toMatchObject({
    method: 'PUT',
    body: JSON.stringify({ visibility: 'public' }),
  });
  expect(new Headers(writes[0]?.headers).get('If-Match')).toBe('4');
  expect(screen.getByText('trusted-account')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Make private' }));
  await screen.findByRole('button', { name: 'Make public' });
  expect(new Headers(writes[1]?.headers).get('If-Match')).toBe('5');
  expect(JSON.parse(writes[1]?.body as string)).toEqual({ visibility: 'private' });
  expect(screen.getByText('trusted-account')).toBeVisible();
});

it('pages the public catalog with its scope and shows version-only responses to visitors', async () => {
  const publicImage = { ...image, owner: 'publisher', shares: [], visibility: 'public' };
  const fetcher = vi.fn(async (url: string) => {
    if (url.endsWith('/versions'))
      return page([
        {
          id: 'version-one',
          phase: 'succeeded',
          reference: 'registry/runtime@sha256:example',
          created_at_ms: 0,
        },
      ]);
    if (url.includes('scope=public&after='))
      return page([{ ...publicImage, id: 'second-image', name: 'Second runtime' }]);
    if (url.endsWith('scope=public')) return page([publicImage], 'image-one');
    return page([]);
  });
  vi.stubGlobal('fetch', fetcher);
  render(
    <ImageManager
      session={{ ...session, permissions: ['cma-im:images:read', 'cma-im:images:use'] }}
      denied={vi.fn()}
    />,
  );
  await chooseOptionValue(userEvent.setup(), screen.getByLabelText('Image catalog'), 'public');
  fireEvent.click(await screen.findByRole('button', { name: 'Open image' }));
  expect(await screen.findByText('registry/runtime@sha256:example')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Make public' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Make private' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'View build logs' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Load more images' }));
  await screen.findByRole('heading', { name: 'Second runtime' });
  expect(fetcher).toHaveBeenCalledWith(
    '/api/image-manager/v1/images?scope=public&after=image-one',
    expect.anything(),
  );
  await chooseOptionValue(userEvent.setup(), screen.getByLabelText('Image catalog'), 'owned');
  await waitFor(() => expect(screen.queryByText('registry/runtime@sha256:example')).toBeNull());
  expect(screen.queryByRole('heading', { name: 'Second runtime' })).toBeNull();
});

it('retains the observed visibility after a refused write', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      if (url.endsWith('/visibility'))
        return new Response(JSON.stringify({ detail: 'Image changed; refresh and retry.' }), {
          status: 409,
        });
      return url.includes('/builds') ? page([]) : page([image]);
    }),
  );
  render(<ImageManager session={session} denied={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Open image' }));
  fireEvent.click(screen.getByText('Sharing'));
  fireEvent.click(screen.getByRole('button', { name: 'Make public' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Image changed; refresh and retry.');
  expect(screen.getByRole('button', { name: 'Make public' })).toBeEnabled();
  expect(screen.queryByRole('button', { name: 'Make private' })).toBeNull();
});
