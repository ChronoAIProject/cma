import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ServicePage } from './ServicePage';
import type { Denied } from './client';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
const session = (subject = 'owner') => ({
  authenticated: true,
  subject,
  permissions: ['cma-eg:channels:read'],
  expires_at: 9999999999,
});
it('opens Event Gateway from the CMA session without an independent sign-in flow', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(session())));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ServicePage service="event-gateway">{(value) => <p>{value.subject} channels</p>}</ServicePage>,
  );
  expect(await screen.findByText('owner channels')).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Sign in with NyxID' })).not.toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Sign out of CMA' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Sign out/ })).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledWith(
    '/api/event-gateway/auth/session',
    expect.objectContaining({ credentials: 'same-origin' }),
  );
});
it('returns a signed-out visitor through the Agent Manager OAuth entry point', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{}', { status: 401 })));
  render(<ServicePage service="event-gateway">{() => <p>private channels</p>}</ServicePage>);
  expect(await screen.findByRole('link', { name: 'Sign in with NyxID' })).toHaveAttribute(
    'href',
    '/auth/login?return_to=%2Fevent-gateway',
  );
  expect(screen.queryByText('private channels')).not.toBeInTheDocument();
});

it('disarms automatic Event Gateway reads after a session 401 until a fresh mount', async () => {
  vi.useFakeTimers();
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response('{}', { status: 401 }))
    .mockImplementation(async () => Response.json(session('new-owner')));
  vi.stubGlobal('fetch', fetcher);
  const portal = (key: string) => (
    <ServicePage key={key} service="event-gateway">
      {(value) => <p>{value.subject} channels</p>}
    </ServicePage>
  );
  const view = render(portal('before-sign-in'));
  await act(async () => {});
  expect(screen.getByRole('link', { name: 'Sign in with NyxID' })).toHaveAttribute(
    'href',
    '/auth/login?return_to=%2Fevent-gateway',
  );
  await act(async () => {
    fireEvent.focus(window);
    fireEvent.focus(window);
    await vi.advanceTimersByTimeAsync(135_000);
  });
  expect(fetcher).toHaveBeenCalledTimes(1);
  expect(screen.queryByText('new-owner channels')).not.toBeInTheDocument();

  view.rerender(portal('after-sign-in'));
  await act(async () => {});
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(screen.getByText('new-owner channels')).toBeInTheDocument();
});

it('aborts a pending Event Gateway session read after protected content reports a 401', async () => {
  vi.useFakeTimers();
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  let resolve!: (response: Response) => void;
  const promise = new Promise<Response>((complete) => {
    resolve = complete;
  });
  const fetcher = vi
    .fn<(url: string, options: RequestInit) => Promise<Response>>()
    .mockResolvedValueOnce(Response.json(session()))
    .mockImplementationOnce(() => promise);
  vi.stubGlobal('fetch', fetcher);
  let reportDenied: Denied | undefined;
  render(
    <ServicePage service="event-gateway">
      {(value, denied) => {
        reportDenied = denied;
        return <button onClick={() => denied(401)}>{value.subject} channels</button>;
      }}
    </ServicePage>,
  );
  await act(async () => {});
  fireEvent.focus(window);
  const pending = fetcher.mock.calls[1]?.[1].signal;
  expect(pending?.aborted).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'owner channels' }));
  expect(screen.getByRole('link', { name: 'Sign in with NyxID' })).toBeInTheDocument();
  expect(pending?.aborted).toBe(true);
  await act(async () => {
    resolve(Response.json(session('late-owner')));
    reportDenied?.(403);
    fireEvent.focus(window);
    await vi.advanceTimersByTimeAsync(90_000);
  });
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole('button', { name: /channels/ })).not.toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Sign in with NyxID' })).toBeInTheDocument();
});
it('keeps authenticated forms mounted and passes a retry notice into the portal frame', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response(JSON.stringify(session())))
    .mockRejectedValueOnce(new TypeError('Session refresh interrupted'))
    .mockResolvedValueOnce(new Response(JSON.stringify(session())));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ServicePage service="event-gateway" renderStatus={(content) => <aside>{content}</aside>}>
      {(_value, _denied, notice) => (
        <main>
          {notice}
          <label>
            Draft
            <input />
          </label>
        </main>
      )}
    </ServicePage>,
  );
  const input = await screen.findByLabelText('Draft');
  fireEvent.change(input, { target: { value: 'Unsaved channel' } });
  await act(async () => {
    fireEvent.focus(window);
  });
  expect(await screen.findByRole('alert')).toHaveTextContent('Session refresh interrupted');
  expect(screen.getByLabelText('Draft')).toBe(input);
  expect(input).toHaveValue('Unsaved channel');
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
  });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Draft')).toBe(input);
});
it('replaces owned content on an account change and removes it when the common session expires', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response(JSON.stringify(session())))
    .mockResolvedValueOnce(new Response(JSON.stringify(session('new-owner'))))
    .mockResolvedValueOnce(new Response('{}', { status: 401 }));
  vi.stubGlobal('fetch', fetcher);
  render(
    <ServicePage service="event-gateway">{(value) => <p>{value.subject} channels</p>}</ServicePage>,
  );
  await screen.findByText('owner channels');
  await act(async () => {
    fireEvent.focus(window);
  });
  expect(await screen.findByText('new-owner channels')).toBeInTheDocument();
  expect(screen.queryByText('owner channels')).not.toBeInTheDocument();
  await act(async () => {
    fireEvent.focus(window);
  });
  await screen.findByRole('link', { name: 'Sign in with NyxID' });
  expect(screen.queryByText('new-owner channels')).not.toBeInTheDocument();
});
it('tells a signed-in account without permissions so instead of offering sign-in again', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response('{}', { status: 403 }))
    .mockResolvedValueOnce(new Response(JSON.stringify(session())))
    .mockResolvedValueOnce(new Response(JSON.stringify(session())));
  vi.stubGlobal('fetch', fetcher);
  render(<ServicePage service="event-gateway">{() => <p>private channels</p>}</ServicePage>);
  expect(await screen.findByRole('alert')).toHaveTextContent(
    "Your account doesn't have Event Gateway permissions.",
  );
  expect(screen.queryByRole('link', { name: 'Sign in with NyxID' })).not.toBeInTheDocument();
  expect(screen.queryByText('private channels')).not.toBeInTheDocument();
  // A later focus check must not quietly restore the refused view.
  await act(async () => {
    fireEvent.focus(window);
  });
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(screen.queryByText('private channels')).not.toBeInTheDocument();
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
  });
  expect(await screen.findByText('private channels')).toBeInTheDocument();
});
it.each([
  [403, "Your account doesn't have Event Gateway permissions.", false],
  [401, 'Sign in with NyxID to open Event Gateway.', true],
  [undefined, 'Sign in with NyxID to open Event Gateway.', true],
] as const)('routes a refusal reported with %s to its own page', async (status, text, signIn) => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify(session()))));
  render(
    <ServicePage service="event-gateway">
      {(_value, denied) => (
        <button type="button" onClick={() => denied(status)}>
          Refused read
        </button>
      )}
    </ServicePage>,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Refused read' }));
  expect(screen.getByText(text)).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Sign in with NyxID' }) !== null).toBe(signIn);
  expect(screen.queryByRole('button', { name: 'Refused read' })).not.toBeInTheDocument();
});
