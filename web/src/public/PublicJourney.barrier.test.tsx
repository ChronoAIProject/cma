import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, type Launch } from '@cma/client';
import { AuthContextProvider } from '../auth/context';
import type { AuthState } from '../auth/types';
import { PUBLIC_PREVIEW, PUBLIC_RECORD, PUBLIC_USER, publicClient } from '../test/public';
import { PublicJourney } from './PublicJourney';
import { createIntent, readIntent, writeIntent } from './intent';

const a = { kind: 'public_trigger', publicId: 'entry-a' } as const;
const b = { kind: 'public_trigger', publicId: 'entry-b' } as const;
const key = 'cma:public-intent';
afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

function setup(state: AuthState = { status: 'authenticated', user: PUBLIC_USER }) {
  const client = publicClient();
  const navigate = vi.fn();
  const provider = {
    current: vi.fn(async () => state),
    signIn: vi.fn(async () => undefined),
    signOut: vi.fn(async () => undefined),
  };
  const tree = (route: typeof a | typeof b) => (
    <StrictMode>
      <AuthContextProvider provider={provider}>
        <PublicJourney client={client} route={route} onNavigate={navigate} />
      </AuthContextProvider>
    </StrictMode>
  );
  const view = render(tree(a));
  return {
    ...view,
    client,
    navigate,
    provider,
    user: userEvent.setup(),
    show: (route: typeof a | typeof b) => view.rerender(tree(route)),
  };
}

it.each([false, true])(
  'preserves unresolved A across same-document B and return, expired=%s',
  async (expired) => {
    const test = setup();
    vi.mocked(test.client.requests.createOrResumeBound).mockRejectedValueOnce(
      new ApiError(503, 'Unknown', 'service_unavailable'),
    );
    await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
    await screen.findByRole('alert');
    const original = readIntent('/t/entry-a')!;
    const bytes = sessionStorage.getItem(key);
    if (expired) vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt + 1);
    test.show(b);
    await screen.findByRole('heading', { name: PUBLIC_PREVIEW.title });
    expect(sessionStorage.getItem(key)).toBe(bytes);
    await test.user.click(screen.getByRole('button', { name: 'Open in CMA' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('earlier request');
    expect(screen.getByRole('link', { name: 'Return to saved request' })).toHaveAttribute(
      'href',
      '/t/entry-a',
    );
    expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeDisabled();
    expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem(key)).toBe(bytes);
    expect(test.navigate).not.toHaveBeenCalled();
    test.show(a);
    if (expired) {
      expect(await screen.findByRole('alert')).toHaveTextContent('original request is saved');
      expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce();
      expect(sessionStorage.getItem(key)).toBe(bytes);
    } else {
      await waitFor(() => expect(test.navigate).toHaveBeenCalledOnce());
      const calls = vi.mocked(test.client.requests.createOrResumeBound).mock.calls;
      expect(calls).toHaveLength(2);
      expect(calls[1]).toEqual(calls[0]);
      expect(sessionStorage.getItem(key)).toBeNull();
    }
  },
);

it.each(['button', 'external gesture'] as const)(
  'blocks anonymous B before sign-in via %s',
  async (source) => {
    const original = { ...createIntent(a), submitted: true, owner: PUBLIC_USER.id };
    writeIntent(original);
    const bytes = sessionStorage.getItem(key);
    const test = setup({ status: 'anonymous' });
    if (source === 'external gesture') {
      vi.mocked(test.client.requests.preview).mockResolvedValue({
        ...PUBLIC_PREVIEW,
        always_review: false,
      });
      vi.mocked(test.client.requests.entryIntent).mockResolvedValue({ accepted: true });
    }
    test.show(b);
    if (source === 'button')
      await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('earlier request');
    expect(screen.getByRole('link', { name: 'Return to saved request' })).toHaveAttribute(
      'href',
      '/t/entry-a',
    );
    await act(async () => undefined);
    expect(test.provider.signIn).not.toHaveBeenCalled();
    expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(key)).toBe(bytes);
  },
);

it('does not turn a corrupt recovery route into a link or a fresh dispatch', async () => {
  const original = {
    ...createIntent(a),
    submitted: true,
    owner: PUBLIC_USER.id,
    path: 'https://example.test/unsafe',
  };
  sessionStorage.setItem(key, JSON.stringify(original));
  const bytes = sessionStorage.getItem(key);
  const test = setup();
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('could not be read safely');
  expect(screen.queryByRole('link', { name: 'Return to saved request' })).not.toBeInTheDocument();
  expect(test.client.requests.createOrResumeBound).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(key)).toBe(bytes);
});

it('returns through the router so an accepted in-memory result survives expiry', async () => {
  let resolve!: (value: Launch) => void;
  const response = new Promise<Launch>((yes) => {
    resolve = yes;
  });
  const test = setup();
  vi.mocked(test.client.requests.createOrResumeBound).mockReturnValueOnce(response);
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  const original = readIntent('/t/entry-a')!;
  test.show(b);
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  await act(async () => resolve(PUBLIC_RECORD));
  expect(test.navigate).not.toHaveBeenCalled();
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt + 1);
  await test.user.click(screen.getByRole('link', { name: 'Return to saved request' }));
  expect(test.navigate).toHaveBeenCalledWith(a);
  test.show(a);
  await waitFor(() =>
    expect(test.navigate).toHaveBeenCalledWith(
      { kind: 'launch', launchId: PUBLIC_RECORD.id },
      { replace: true },
    ),
  );
  expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(sessionStorage.getItem(key)).toBeNull();
});

it('retains and fences the original command after a fingerprint mismatch, including re-entry', async () => {
  const test = setup();
  vi.mocked(test.client.requests.createOrResumeBound).mockRejectedValueOnce(
    new ApiError(422, 'Mismatch', 'idempotency_key_reused'),
  );
  await test.user.click(await screen.findByRole('button', { name: 'Open in CMA' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('check the original result');
  expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeDisabled();
  const original = readIntent('/t/entry-a')!;
  expect(original.recoveryRequired).toBe(true);
  expect(original.wireBody).toBe(JSON.stringify(original.body));
  expect(original.key).toBe(
    vi.mocked(test.client.requests.createOrResumeBound).mock.calls[0]?.[2].idempotencyKey,
  );
  test.show(b);
  test.show(a);
  expect(await screen.findByRole('alert')).toHaveTextContent('original request is saved');
  expect(test.client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(readIntent('/t/entry-a')).toEqual(original);
  expect(test.navigate).not.toHaveBeenCalled();
});
