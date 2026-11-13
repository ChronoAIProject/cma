import { StrictMode } from 'react';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { App } from '../App';
import { CmaClientProvider } from '../api/context';
import { createRestWebClient } from '../api/rest';
import { AuthContextProvider } from '../auth/context';
import { createNyxidAuthProvider } from '../auth/nyxid';
import { PUBLIC_PREVIEW } from '../test/public';
import { publicCopy } from '../vocabulary/needs';
import { createPublicClient } from './client';
import { createIntent } from './intent';

// Exercise the real App, auth adapter and SDK decoders. Only HTTP is replaced;
// unexpected requests are recorded even when the consumer catches their errors.
const previewPath = '/api/v1/triggers/public/public-example';
const route = { kind: 'public_trigger', publicId: 'public-example' } as const;
const unexpected: string[] = [];

function fixture(
  options: {
    signedIn?: boolean;
    retired?: boolean;
    malformedPreview?: boolean;
    signupUnavailable?: boolean;
    identityUnavailable?: boolean;
    noReview?: boolean;
  } = {},
) {
  const calls: string[] = [];
  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  const transport = vi.fn<typeof fetch>(async (input, init) => {
    const url = new URL(input instanceof Request ? input.url : String(input), location.origin);
    const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
    const call = `${method} ${url.pathname}${url.search}`;
    calls.push(call);
    if (url.origin === location.origin && method === 'GET') {
      if (url.pathname === '/auth/session') {
        if (options.identityUnavailable) return json({ code: 'identity_unavailable' }, 503);
        return json(
          options.signedIn
            ? {
                authenticated: true,
                subject: 'fixture-visitor',
                visitor: true,
                capabilities: ['launch_triggers'],
              }
            : { authenticated: false },
        );
      }
      if (url.pathname === previewPath)
        return json(
          options.malformedPreview
            ? { title: PUBLIC_PREVIEW.title }
            : {
                ...PUBLIC_PREVIEW,
                always_review: !options.noReview,
                availability: options.retired ? 'retired' : 'published',
              },
        );
      if (url.pathname === '/auth/signup-options')
        return options.signupUnavailable
          ? json({}, 503)
          : json({
              social_providers: [],
              email_signup: true,
              invite_code_required: false,
              register_available: true,
            });
    }
    if (options.noReview && call === `POST ${previewPath}/entry-intent`)
      throw new TypeError('Local read-only guard refused the entry probe');
    unexpected.push(call);
    throw new Error('Unexpected HTTP operation in passive contract');
  });
  vi.stubGlobal('fetch', transport);
  const sdk = createClient({ fetch: transport, requestIdProvider: () => 'passive-contract' });
  const provider = createNyxidAuthProvider();
  const tree = (
    <StrictMode>
      <AuthContextProvider provider={provider}>
        <CmaClientProvider client={createRestWebClient(sdk)} publicClient={createPublicClient(sdk)}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>
    </StrictMode>
  );
  return { calls, mount: () => render(tree) };
}

afterEach(() => {
  cleanup();
  history.replaceState(null, '', '/');
  sessionStorage.clear();
  vi.unstubAllGlobals();
  expect(unexpected.splice(0)).toEqual([]);
});

it.each([false, true])(
  'reviewed landing stays GET-only across remount and identity refresh (signed in: %s)',
  async (signedIn) => {
    history.replaceState(null, '', '/t/public-example');
    const test = fixture({ signedIn });
    for (let pass = 0; pass < 2; pass++) {
      const view = test.mount();
      expect(await screen.findByRole('heading', { name: PUBLIC_PREVIEW.title })).toBeVisible();
      await waitFor(() =>
        expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeEnabled(),
      );
      if (!signedIn)
        expect(await screen.findByRole('button', { name: 'Create account' })).toBeEnabled();
      await act(async () => window.dispatchEvent(new Event('focus')));
      view.unmount();
    }
    const expected = ['GET /auth/session', `GET ${previewPath}`];
    if (!signedIn) expected.push('GET /auth/signup-options');
    expect([...new Set(test.calls)].sort()).toEqual(expected.sort());
    expect(sessionStorage.getItem('cma:public-intent')).toBeNull();
  },
);

it('anonymous console mounts only the passive session read', async () => {
  history.replaceState(null, '', '/');
  const test = fixture();
  test.mount();
  expect(await screen.findByRole('button', { name: 'Sign in' })).toBeEnabled();
  expect(new Set(test.calls)).toEqual(new Set(['GET /auth/session']));
});

it('retired preview cannot consume a retained visitor intent or admit work', async () => {
  history.replaceState(null, '', '/t/public-example');
  createIntent(route);
  const test = fixture({ signedIn: true, retired: true });
  test.mount();
  expect(await screen.findByText(publicCopy.unavailable)).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Open in CMA' })).not.toBeInTheDocument();
  expect(new Set(test.calls)).toEqual(new Set(['GET /auth/session', `GET ${previewPath}`]));
});

it('malformed public preview exposes a retryable error without signup or admission', async () => {
  history.replaceState(null, '', '/t/public-example');
  const test = fixture({ malformedPreview: true });
  test.mount();
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Open in CMA' })).not.toBeInTheDocument();
  expect(new Set(test.calls)).toEqual(new Set(['GET /auth/session', `GET ${previewPath}`]));
});

it('signup discovery failure leaves sign-in available without inventing registration authority', async () => {
  history.replaceState(null, '', '/t/public-example');
  const test = fixture({ signupUnavailable: true });
  test.mount();
  expect(await screen.findByText(publicCopy.signupUnavailable)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeEnabled();
  expect(screen.queryByRole('button', { name: 'Create account' })).not.toBeInTheDocument();
  expect(test.calls.every((call) => call.startsWith('GET '))).toBe(true);
});

it('identity failure cannot turn a retained intent into an admission', async () => {
  history.replaceState(null, '', '/t/public-example');
  createIntent(route);
  const test = fixture({ identityUnavailable: true });
  test.mount();
  expect(
    await screen.findByRole('heading', { name: 'The identity service is temporarily unavailable' }),
  ).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Open in CMA' })).not.toBeInTheDocument();
  expect(test.calls.every((call) => call.startsWith('GET '))).toBe(true);
});

it('no-review landing needs a POST entry probe and a refused probe never admits work', async () => {
  history.replaceState(null, '', '/t/public-example');
  const test = fixture({ signedIn: true, noReview: true });
  test.mount();
  await waitFor(() => expect(test.calls).toContain(`POST ${previewPath}/entry-intent`));
  expect(screen.getByRole('button', { name: 'Open in CMA' })).toBeEnabled();
  expect(test.calls.filter((call) => call.startsWith('POST '))).toEqual([
    `POST ${previewPath}/entry-intent`,
  ]);
  expect(sessionStorage.getItem('cma:public-intent')).toBeNull();
});
